//! Write operations and their operation-specific failures.
use super::Step;
use crate::{
    Write as WriteHost,
    error::{self, WriteZero},
};
use bytes::Buf;
use core::{
    convert::Infallible,
    ops::CoroutineState::Complete,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::CallOn;
use snafu::{IntoError, Snafu};

/// A write completion returns the remaining input even on error.
#[derive(Debug)]
pub struct WriteReturn<B, E> {
    pub input: B,
    pub result: Result<usize, E>,
}

/// One write, owning the input value (which may itself be a mutable borrow).
pub struct Write<B>(Option<B>);
// The input is movable data, never a structurally pinned field.
impl<B> Unpin for Write<B> {}
impl<B: Buf> Write<B> {
    pub fn new(input: B) -> Self {
        Self(Some(input))
    }
}
impl<H: WriteHost, B: Buf> CallOn<H> for Write<B> {
    type Yield = Infallible;
    type Return = WriteReturn<B, H::Error>;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return>
    where
        H: 'visit,
    {
        let host = ready!(host.poll_view(cx));
        let input = &mut self.get_mut().0;
        let result = ready!(H::poll_write(
            host,
            cx,
            input.as_mut().expect("completed write")
        ));
        Poll::Ready(Complete(WriteReturn {
            input: input.take().unwrap(),
            result,
        }))
    }
}

/// Accept all input, preserving the unaccepted tail and count on failure.
pub struct WriteAll<B> {
    input: Option<B>,
    completed: usize,
}

/// Failures that [`WriteAll`](crate::calls::WriteAll) can produce.
#[derive(Debug, Snafu)]
#[snafu(module(write_all), visibility(pub(crate)))]
pub enum WriteAllError<E> {
    #[snafu(transparent)]
    WriteZero { source: WriteZero },
    #[snafu(display("write failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> WriteAllError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::WriteZero { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::WriteZero { .. } => None,
        }
    }
}
impl<B> Unpin for WriteAll<B> {}
impl<B: Buf> WriteAll<B> {
    pub fn new(input: B) -> Self {
        Self {
            input: Some(input),
            completed: 0,
        }
    }
}
impl<H: WriteHost, B: Buf> CallOn<H> for WriteAll<B> {
    type Yield = Infallible;
    type Return = WriteReturn<B, WriteAllError<H::Error>>;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        mut host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return>
    where
        H: 'visit,
    {
        let this = self.get_mut();
        let result = loop {
            let input = this.input.as_mut().expect("completed write all");
            if !input.has_remaining() {
                break Ok(this.completed);
            }
            let before = input.remaining();
            match ready!(H::poll_write(
                ready!(host.as_mut().poll_view(cx)),
                cx,
                input
            )) {
                Ok(0) => {
                    break Err(error::WriteZeroSnafu {
                        completed: this.completed,
                    }
                    .build()
                    .into());
                }
                Ok(count) => {
                    assert_eq!(
                        before.checked_sub(input.remaining()),
                        Some(count),
                        "write cursor/count mismatch"
                    );
                    this.completed += count;
                }
                Err(source) => {
                    break Err(write_all::ReceiverSnafu {
                        completed: this.completed,
                    }
                    .into_error(source));
                }
            }
        };
        Poll::Ready(Complete(WriteReturn {
            input: this.input.take().unwrap(),
            result,
        }))
    }
}

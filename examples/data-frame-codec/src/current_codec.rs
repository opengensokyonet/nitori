//! Current named codec operations using the temporary resume environment.
use bytes::{Buf, Bytes};
use nitori_call::CallOn;
use nitori_call::call;
use snafu::Snafu;
use std::num::NonZeroUsize;
use std::{
    convert::Infallible,
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Debug, Snafu)]
pub enum CodecError {
    #[snafu(display("unexpected end of frame"))]
    Eof,
    #[snafu(display("wrong DATA frame type"))]
    WrongType,
    #[snafu(display("input/output failed"))]
    Io,
}
pub trait ReadSource {
    fn poll_read(
        self: Pin<&mut Self>,
        maximum: NonZeroUsize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Bytes>, CodecError>>;
}
pub struct ReadAtMost(NonZeroUsize);
impl ReadAtMost {
    pub fn new(maximum: NonZeroUsize) -> Self {
        Self(maximum)
    }
}

impl<Target: ReadSource + ?Sized> CallOn<Target> for ReadAtMost {
    type Yield = Infallible;
    type Return = Result<Option<Bytes>, CodecError>;
    fn poll_call(
        self: Pin<&mut Self>,
        target: Pin<&mut Target>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>> {
        target.poll_read(self.0, cx).map(CoroutineState::Complete)
    }
}

pub trait WriteSink<Input: ?Sized> {
    fn poll_write(
        self: Pin<&mut Self>,
        input: &mut Input,
        cx: &mut Context<'_>,
    ) -> Poll<Result<usize, CodecError>>;
}
pub struct Write<Input>(Option<Input>);
// Input is a movable message, never a structurally pinned field.
impl<Input> Unpin for Write<Input> {}
impl<Input> Write<Input> {
    pub fn new(input: Input) -> Self {
        Self(Some(input))
    }
}
pub struct WriteReturn<Input> {
    pub input: Input,
    pub result: Result<usize, CodecError>,
}

impl<Target: WriteSink<Input> + ?Sized, Input: Buf> CallOn<Target> for Write<Input> {
    type Yield = Infallible;
    type Return = WriteReturn<Input>;
    fn poll_call(
        self: Pin<&mut Self>,
        target: Pin<&mut Target>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>> {
        let input = &mut self.get_mut().0;
        let before = input.as_ref().expect("write input").remaining();
        let result = target.poll_write(input.as_mut().unwrap(), cx);
        let after = input.as_ref().unwrap().remaining();
        match result {
            Poll::Pending => {
                assert_eq!(before, after);
                Poll::Pending
            }
            Poll::Ready(result) => {
                match &result {
                    Ok(count) => {
                        assert_eq!(before.checked_sub(after), Some(*count));
                        assert!(before == 0 || *count > 0);
                    }
                    Err(_) => assert_eq!(before, after),
                }
                Poll::Ready(CoroutineState::Complete(WriteReturn {
                    input: input.take().unwrap(),
                    result,
                }))
            }
        }
    }
}

#[call]
pub async fn read_varint<Target: ReadSource + ?Sized>(
    io: nitori_call::Receiver<'_, Target>,
) -> Result<u64, CodecError> {
    let mut first = io
        .read_at_most(NonZeroUsize::new(1).unwrap())
        .await?
        .ok_or(CodecError::Eof)?;
    let byte = first.get_u8();
    drop(first);
    let mut remaining = (1usize << (byte >> 6)) - 1;
    let mut value = u64::from(byte & 63);
    while remaining != 0 {
        let mut part = io
            .read_at_most(NonZeroUsize::new(remaining).unwrap())
            .await?
            .ok_or(CodecError::Eof)?;
        remaining -= part.remaining();
        while part.has_remaining() {
            value = (value << 8) | u64::from(part.get_u8());
        }
    }
    Ok(value)
}

#[derive(Debug)]
pub struct DataChunk {
    pub bytes: Bytes,
    pub remaining: u64,
}

#[call(yields = DataChunk)]
pub async fn read_data_frame<Target: ReadSource + ?Sized>(
    io: nitori_call::Receiver<'_, Target>,
    quantum: NonZeroUsize,
) -> Result<u64, CodecError> {
    let frame_type = io.read_varint().await?;
    if frame_type != 0 {
        return Err(CodecError::WrongType);
    }
    let length = io.read_varint().await?;
    let mut remaining = length;
    while remaining != 0 {
        let maximum = NonZeroUsize::new(remaining.min(quantum.get() as u64) as usize).unwrap();
        let bytes = io.read_at_most(maximum).await?.ok_or(CodecError::Eof)?;
        remaining -= bytes.len() as u64;
        yield DataChunk { bytes, remaining };
    }
    Ok(length)
}

#[call]
pub async fn write_all<Target: WriteSink<Input> + ?Sized, Input: Buf>(
    io: nitori_call::Receiver<'_, Target>,
    mut input: Input,
) -> WriteReturn<Input> {
    let mut written = 0;
    while input.has_remaining() {
        let returned = io.write(input).await;
        input = returned.input;
        match returned.result {
            Ok(count) => written += count,
            Err(error) => {
                return WriteReturn {
                    input,
                    result: Err(error),
                };
            }
        }
    }
    WriteReturn {
        input,
        result: Ok(written),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nitori_call::CallOn;
    use std::{
        ops::CoroutineState,
        pin::{Pin, pin},
        task::{Context, Poll, Waker},
    };

    struct Input(Bytes);
    impl ReadSource for Input {
        fn poll_read(
            mut self: Pin<&mut Self>,
            maximum: NonZeroUsize,
            _: &mut Context<'_>,
        ) -> Poll<Result<Option<Bytes>, CodecError>> {
            let count = maximum.get().min(self.0.len());
            Poll::Ready(Ok(if count == 0 {
                None
            } else {
                Some(self.0.split_to(count))
            }))
        }
    }
    #[test]
    fn named_frame_obeys_chunk_demand_and_preserves_next_frame() {
        let mut input = Input(Bytes::from_static(b"\0\x03abc\0\0"));
        let mut operation = pin!(read_data_frame::<Input>(NonZeroUsize::new(2).unwrap()));
        let mut cx = Context::from_waker(Waker::noop());
        for (expected, remaining) in [(b"ab".as_slice(), 1), (b"c".as_slice(), 0)] {
            let Poll::Ready(CoroutineState::Yielded(chunk)) =
                operation.as_mut().poll_call(Pin::new(&mut input), &mut cx)
            else {
                panic!("expected chunk")
            };
            assert_eq!(chunk.bytes.as_ref(), expected);
            assert_eq!(chunk.remaining, remaining);
            assert_eq!(input.0.len(), remaining as usize + 2);
        }
        assert!(matches!(
            operation.as_mut().poll_call(Pin::new(&mut input), &mut cx),
            Poll::Ready(CoroutineState::Complete(Ok(3)))
        ));
        assert_eq!(input.0.as_ref(), b"\0\0");
    }
    struct Output {
        bytes: Vec<u8>,
        waited: bool,
    }
    impl WriteSink<Bytes> for Output {
        fn poll_write(
            mut self: Pin<&mut Self>,
            input: &mut Bytes,
            cx: &mut Context<'_>,
        ) -> Poll<Result<usize, CodecError>> {
            if !self.waited {
                self.waited = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.waited = false;
            let count = input.len().min(2);
            self.bytes.extend_from_slice(&input[..count]);
            input.advance(count);
            Poll::Ready(Ok(count))
        }
    }
    #[test]
    fn named_write_preserves_partial_progress_across_pending() {
        let mut output = Output {
            bytes: Vec::new(),
            waited: false,
        };
        let mut operation = pin!(write_all::<Output, _>(Bytes::from_static(b"abc")));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            operation
                .as_mut()
                .poll_call(Pin::new(&mut output), &mut cx)
                .is_pending()
        );
        assert!(output.bytes.is_empty());
        assert!(
            operation
                .as_mut()
                .poll_call(Pin::new(&mut output), &mut cx)
                .is_pending()
        );
        assert_eq!(output.bytes, b"ab");
        let Poll::Ready(CoroutineState::Complete(returned)) =
            operation.as_mut().poll_call(Pin::new(&mut output), &mut cx)
        else {
            panic!("expected completion")
        };
        assert_eq!(returned.result.unwrap(), 3);
        assert!(returned.input.is_empty());
        assert_eq!(output.bytes, b"abc");
    }
}

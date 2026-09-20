//! Operations for `CallOn`. Import their type names at `#[call]` use sites.
//!
//! Constructors only capture arguments; all checks and IO happen when polled.
//! A completed operation must not be polled again. Derived operations preserve
//! completed prefixes on Pending, errors, and cancellation; they never roll back.
use crate::{
    Read as ReadHost, ReadChunk as ChunkHost, Write as WriteHost,
    error::{ReadError, WriteError},
};
use bytes::{Buf, BufMut};
use core::{
    convert::Infallible,
    marker::PhantomData,
    num::NonZeroUsize,
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::CallOn;

type Step<Y, R> = Poll<CoroutineState<Y, R>>;
use CoroutineState::{Complete, Yielded};

/// One fill operation. The destination borrow can survive Pending.
pub struct Read<'a, B: ?Sized>(&'a mut B);
impl<'a, B: BufMut + ?Sized> Read<'a, B> {
    pub fn new(destination: &'a mut B) -> Self {
        Self(destination)
    }
}
impl<H: ReadHost + ?Sized, B: BufMut + ?Sized> CallOn<H> for Read<'_, B> {
    type Yield = Infallible;
    type Return = Result<usize, H::Error>;
    fn poll_call(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return> {
        host.poll_read(cx, self.get_mut().0).map(Complete)
    }
}

/// One owned chunk or EOF.
pub struct ReadChunk(NonZeroUsize);
impl ReadChunk {
    pub fn new(maximum: NonZeroUsize) -> Self {
        Self(maximum)
    }
}
impl<H: ChunkHost + ?Sized> CallOn<H> for ReadChunk {
    type Yield = Infallible;
    type Return = Result<Option<H::Chunk>, H::Error>;
    fn poll_call(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return> {
        host.poll_read_chunk(cx, self.0).map(Complete)
    }
}

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
impl<H: WriteHost + ?Sized, B: Buf> CallOn<H> for Write<B> {
    type Yield = Infallible;
    type Return = WriteReturn<B, H::Error>;
    fn poll_call(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return> {
        let input = &mut self.get_mut().0;
        let result = ready!(host.poll_write(cx, input.as_mut().expect("completed write")));
        Poll::Ready(Complete(WriteReturn {
            input: input.take().unwrap(),
            result,
        }))
    }
}

/// Fill exactly `length` bytes, checking the whole capacity before any IO.
pub struct ReadExact<'a, B: ?Sized> {
    destination: &'a mut B,
    length: usize,
    completed: usize,
    checked: bool,
}
impl<'a, B: BufMut + ?Sized> ReadExact<'a, B> {
    pub fn new(destination: &'a mut B, length: usize) -> Self {
        Self {
            destination,
            length,
            completed: 0,
            checked: false,
        }
    }
}
impl<H: ReadHost + ?Sized, B: BufMut + ?Sized> CallOn<H> for ReadExact<'_, B> {
    type Yield = Infallible;
    type Return = Result<(), ReadError<H::Error>>;
    fn poll_call(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return> {
        let this = self.get_mut();
        if !this.checked {
            this.checked = true;
            let available = this.destination.remaining_mut();
            if available < this.length {
                return Poll::Ready(Complete(Err(ReadError::capacity(
                    this.length,
                    available,
                    0,
                ))));
            }
        }
        poll_exact(host, cx, this.destination, this.length, &mut this.completed).map(Complete)
    }
}

fn poll_exact<H: ReadHost + ?Sized, B: BufMut + ?Sized>(
    mut host: Pin<&mut H>,
    cx: &mut Context<'_>,
    mut destination: &mut B,
    length: usize,
    completed: &mut usize,
) -> Poll<Result<(), ReadError<H::Error>>> {
    while *completed < length {
        let remaining = length - *completed;
        let result = ready!(
            host.as_mut()
                .poll_read(cx, &mut (&mut destination).limit(remaining))
        );
        match result {
            Ok(0) => return Poll::Ready(Err(ReadError::unexpected_eof(length, *completed))),
            Ok(count) => {
                assert!(count <= remaining, "read exceeded limit");
                *completed += count;
            }
            Err(source) => return Poll::Ready(Err(ReadError::host(source, *completed))),
        }
    }
    Poll::Ready(Ok(()))
}

/// Append until EOF. A full destination before confirmed EOF is an error.
pub struct ReadToEnd<'a, B: ?Sized> {
    destination: &'a mut B,
    completed: usize,
}
impl<'a, B: BufMut + ?Sized> ReadToEnd<'a, B> {
    pub fn new(destination: &'a mut B) -> Self {
        Self {
            destination,
            completed: 0,
        }
    }
}
impl<H: ReadHost + ?Sized, B: BufMut + ?Sized> CallOn<H> for ReadToEnd<'_, B> {
    type Yield = Infallible;
    type Return = Result<usize, ReadError<H::Error>>;
    fn poll_call(
        self: Pin<&mut Self>,
        mut host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return> {
        let this = self.get_mut();
        loop {
            if this.destination.remaining_mut() == 0 {
                return Poll::Ready(Complete(Err(ReadError::capacity(1, 0, this.completed))));
            }
            match ready!(host.as_mut().poll_read(cx, this.destination)) {
                Ok(0) => return Poll::Ready(Complete(Ok(this.completed))),
                Ok(count) => {
                    this.completed = this
                        .completed
                        .checked_add(count)
                        .expect("read byte count overflow")
                }
                Err(source) => {
                    return Poll::Ready(Complete(Err(ReadError::host(source, this.completed))));
                }
            }
        }
    }
}

/// Yield chunks until the byte maximum or EOF; completion returns total bytes.
pub struct ReadChunks {
    maximum: usize,
    chunk_maximum: NonZeroUsize,
    completed: usize,
}
impl ReadChunks {
    pub fn new(maximum: usize, chunk_maximum: NonZeroUsize) -> Self {
        Self {
            maximum,
            chunk_maximum,
            completed: 0,
        }
    }
}
impl<H: ChunkHost + ?Sized> CallOn<H> for ReadChunks {
    type Yield = H::Chunk;
    type Return = Result<usize, ReadError<H::Error>>;
    fn poll_call(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return> {
        let this = self.get_mut();
        let Some(maximum) =
            NonZeroUsize::new((this.maximum - this.completed).min(this.chunk_maximum.get()))
        else {
            return Poll::Ready(Complete(Ok(this.completed)));
        };
        match ready!(host.poll_read_chunk(cx, maximum)) {
            Ok(Some(chunk)) => {
                let count = chunk.remaining();
                assert!(count > 0 && count <= maximum.get(), "invalid chunk length");
                this.completed += count;
                Poll::Ready(Yielded(chunk))
            }
            Ok(None) => Poll::Ready(Complete(Ok(this.completed))),
            Err(source) => Poll::Ready(Complete(Err(ReadError::host(source, this.completed)))),
        }
    }
}

/// Yield exactly `length` bytes, reporting premature EOF with prior progress.
pub struct ReadChunksExact(ReadChunks);
impl ReadChunksExact {
    pub fn new(length: usize, chunk_maximum: NonZeroUsize) -> Self {
        Self(ReadChunks::new(length, chunk_maximum))
    }
}
impl<H: ChunkHost + ?Sized> CallOn<H> for ReadChunksExact {
    type Yield = H::Chunk;
    type Return = Result<(), ReadError<H::Error>>;
    fn poll_call(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return> {
        let this = self.get_mut();
        Pin::new(&mut this.0)
            .poll_call(host, cx)
            .map(|state| match state {
                Yielded(chunk) => Yielded(chunk),
                Complete(Err(error)) => Complete(Err(error)),
                Complete(Ok(completed)) if completed == this.0.maximum => Complete(Ok(())),
                Complete(Ok(completed)) => {
                    Complete(Err(ReadError::unexpected_eof(this.0.maximum, completed)))
                }
            })
    }
}

/// Accept all input, preserving the unaccepted tail and count on failure.
pub struct WriteAll<B> {
    input: Option<B>,
    completed: usize,
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
impl<H: WriteHost + ?Sized, B: Buf> CallOn<H> for WriteAll<B> {
    type Yield = Infallible;
    type Return = WriteReturn<B, WriteError<H::Error>>;
    fn poll_call(
        self: Pin<&mut Self>,
        mut host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return> {
        let this = self.get_mut();
        let result = loop {
            let input = this.input.as_mut().expect("completed write all");
            if !input.has_remaining() {
                break Ok(this.completed);
            }
            let before = input.remaining();
            match ready!(host.as_mut().poll_write(cx, input)) {
                Ok(0) => break Err(WriteError::write_zero(this.completed)),
                Ok(count) => {
                    assert_eq!(
                        before.checked_sub(input.remaining()),
                        Some(count),
                        "write cursor/count mismatch"
                    );
                    this.completed += count;
                }
                Err(source) => break Err(WriteError::host(source, this.completed)),
            }
        };
        Poll::Ready(Complete(WriteReturn {
            input: this.input.take().unwrap(),
            result,
        }))
    }
}

/// Read a fixed-size array without selecting a chunk representation.
pub struct ReadArray<const LENGTH: usize> {
    bytes: [u8; LENGTH],
    completed: usize,
}
impl<const LENGTH: usize> Default for ReadArray<LENGTH> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const LENGTH: usize> ReadArray<LENGTH> {
    pub fn new() -> Self {
        Self {
            bytes: [0; LENGTH],
            completed: 0,
        }
    }
}
impl<H: ReadHost + ?Sized, const LENGTH: usize> CallOn<H> for ReadArray<LENGTH> {
    type Yield = Infallible;
    type Return = Result<[u8; LENGTH], ReadError<H::Error>>;
    fn poll_call(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return> {
        let this = self.get_mut();
        let mut destination = &mut this.bytes[this.completed..];
        let result = ready!(poll_exact(
            host,
            cx,
            &mut destination,
            LENGTH,
            &mut this.completed
        ));
        Poll::Ready(Complete(result.map(|()| this.bytes)))
    }
}

/// Decode a primitive number in little endian after reading its complete array.
pub struct ReadLe<T: EndianValue> {
    array: T::ArrayCall,
    marker: PhantomData<fn() -> T>,
}
/// Decode a primitive number in big endian after reading its complete array.
pub struct ReadBe<T: EndianValue> {
    array: T::ArrayCall,
    marker: PhantomData<fn() -> T>,
}

/// Sealed implementation detail selecting a concrete array for each number.
/// Implemented for integers (including pointer-sized integers), f32, and f64.
pub trait EndianValue: private::Sealed {
    #[doc(hidden)]
    type ArrayCall: Default + Unpin;
}
mod private {
    pub trait Sealed {}
}
macro_rules! endian_constructors {
    ($name:ident) => {
        impl<T: EndianValue> Default for $name<T> {
            fn default() -> Self {
                Self::new()
            }
        }
        impl<T: EndianValue> $name<T> {
            pub fn new() -> Self {
                Self {
                    array: Default::default(),
                    marker: PhantomData,
                }
            }
        }
    };
}
endian_constructors!(ReadLe);
endian_constructors!(ReadBe);
macro_rules! endian_impl {
    ($($number:ty),* $(,)?) => {$ (
        impl private::Sealed for $number {}
        impl EndianValue for $number { type ArrayCall = ReadArray<{ core::mem::size_of::<Self>() }>; }
        endian_impl!(@call ReadLe, read_le, $number);
        endian_impl!(@call ReadBe, read_be, $number);
    )*};
    (@call $name:ident, $method:ident, $number:ty) => {
        impl<H: ReadHost + ?Sized> CallOn<H> for $name<$number> {
            type Yield = Infallible;
            type Return = Result<$number, ReadError<H::Error>>;
            fn poll_call(self: Pin<&mut Self>, host: Pin<&mut H>, cx: &mut Context<'_>) -> Step<Self::Yield, Self::Return> {
                use std::io::Read as _;
                Pin::new(&mut self.get_mut().array).poll_call(host, cx).map(|state| match state {
                    Yielded(never) => match never {},
                    Complete(result) => Complete(result.map(|bytes| bytes.as_slice().$method::<$number>().expect("complete numeric array"))),
                })
            }
        }
    };
}
endian_impl!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);

//! Read operations and their operation-specific failures.
use super::Step;
use crate::{
    Read as ReadHost, ReadChunk as ChunkHost,
    error::{self, Incomplete, InsufficientCapacity},
};
use bytes::{Buf, BufMut};
use core::{
    convert::Infallible,
    marker::PhantomData,
    num::NonZeroUsize,
    ops::CoroutineState::{Complete, Yielded},
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::CallOn;
use snafu::{IntoError, Snafu};

/// One fill operation. The destination borrow can survive Pending.
pub struct Read<'a, B: ?Sized>(&'a mut B);
impl<'a, B: BufMut + ?Sized> Read<'a, B> {
    pub fn new(destination: &'a mut B) -> Self {
        Self(destination)
    }
}
impl<H: ReadHost, B: BufMut + ?Sized> CallOn<H> for Read<'_, B> {
    type Yield = Infallible;
    type Return = Result<usize, H::Error>;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return>
    where
        H: 'visit,
    {
        let host = ready!(host.poll_view(cx));
        H::poll_read(host, cx, self.get_mut().0).map(Complete)
    }
}

/// One owned chunk or EOF.
pub struct ReadChunk(NonZeroUsize);
impl ReadChunk {
    pub fn new(maximum: NonZeroUsize) -> Self {
        Self(maximum)
    }
}
impl<H: ChunkHost> CallOn<H> for ReadChunk {
    type Yield = Infallible;
    type Return = Result<Option<H::Chunk>, H::Error>;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return>
    where
        H: 'visit,
    {
        let host = ready!(host.poll_view(cx));
        H::poll_read_chunk(host, cx, self.0).map(Complete)
    }
}

/// Fill exactly `length` bytes, checking the whole capacity before any IO.
pub struct ReadExact<'a, B: ?Sized> {
    destination: &'a mut B,
    length: usize,
    completed: usize,
    checked: bool,
}

/// Failures that [`ReadExact`](crate::calls::ReadExact) can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_exact), visibility(pub(crate)))]
pub enum ReadExactError<E> {
    #[snafu(transparent)]
    InsufficientCapacity { source: InsufficientCapacity },
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadExactError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::InsufficientCapacity { source } => source.completed,
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::InsufficientCapacity { .. } | Self::Incomplete { .. } => None,
        }
    }
}

impl<E> From<ReadArrayError<E>> for ReadExactError<E> {
    fn from(error: ReadArrayError<E>) -> Self {
        match error {
            ReadArrayError::Incomplete { source } => source.into(),
            ReadArrayError::Receiver { source, completed } => {
                read_exact::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
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
impl<H: ReadHost, B: BufMut + ?Sized> CallOn<H> for ReadExact<'_, B> {
    type Yield = Infallible;
    type Return = Result<(), ReadExactError<H::Error>>;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return>
    where
        H: 'visit,
    {
        let this = self.get_mut();
        if !this.checked {
            this.checked = true;
            let available = this.destination.remaining_mut();
            if available < this.length {
                return Poll::Ready(Complete(Err(error::InsufficientCapacitySnafu {
                    required: this.length,
                    available,
                    completed: 0usize,
                }
                .build()
                .into())));
            }
        }
        if this.completed == this.length {
            return Poll::Ready(Complete(Ok(())));
        }
        let host = ready!(host.poll_view(cx));
        poll_exact::<H, _>(host, cx, this.destination, this.length, &mut this.completed)
            .map(|result| Complete(result.map_err(ReadExactError::from)))
    }
}

fn poll_exact<'visit, H: ReadHost + 'visit, B: BufMut + ?Sized>(
    mut host: Pin<&mut H::ReceiverView<'visit>>,
    cx: &mut Context<'_>,
    mut destination: &mut B,
    length: usize,
    completed: &mut usize,
) -> Poll<Result<(), ReadArrayError<H::Error>>> {
    while *completed < length {
        let remaining = length - *completed;
        let result = ready!(H::poll_read(
            host.as_mut(),
            cx,
            &mut (&mut destination).limit(remaining)
        ));
        match result {
            Ok(0) => {
                return Poll::Ready(Err(error::IncompleteSnafu {
                    expected: length,
                    completed: *completed,
                }
                .build()
                .into()));
            }
            Ok(count) => {
                assert!(count <= remaining, "read exceeded limit");
                *completed += count;
            }
            Err(source) => {
                return Poll::Ready(Err(read_array::ReceiverSnafu {
                    completed: *completed,
                }
                .into_error(source)));
            }
        }
    }
    Poll::Ready(Ok(()))
}

/// Append until EOF. A full destination before confirmed EOF is an error.
pub struct ReadToEnd<'a, B: ?Sized> {
    destination: &'a mut B,
    completed: usize,
}

/// Failures that [`ReadToEnd`](crate::calls::ReadToEnd) can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_to_end), visibility(pub(crate)))]
pub enum ReadToEndError<E> {
    #[snafu(transparent)]
    InsufficientCapacity { source: InsufficientCapacity },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadToEndError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::InsufficientCapacity { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::InsufficientCapacity { .. } => None,
        }
    }
}
impl<'a, B: BufMut + ?Sized> ReadToEnd<'a, B> {
    pub fn new(destination: &'a mut B) -> Self {
        Self {
            destination,
            completed: 0,
        }
    }
}
impl<H: ReadHost, B: BufMut + ?Sized> CallOn<H> for ReadToEnd<'_, B> {
    type Yield = Infallible;
    type Return = Result<usize, ReadToEndError<H::Error>>;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        mut host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return>
    where
        H: 'visit,
    {
        let this = self.get_mut();
        loop {
            if this.destination.remaining_mut() == 0 {
                return Poll::Ready(Complete(Err(error::InsufficientCapacitySnafu {
                    required: 1usize,
                    available: 0usize,
                    completed: this.completed,
                }
                .build()
                .into())));
            }
            match ready!(H::poll_read(
                ready!(host.as_mut().poll_view(cx)),
                cx,
                this.destination
            )) {
                Ok(0) => return Poll::Ready(Complete(Ok(this.completed))),
                Ok(count) => {
                    this.completed = this
                        .completed
                        .checked_add(count)
                        .expect("read byte count overflow")
                }
                Err(source) => {
                    return Poll::Ready(Complete(Err(read_to_end::ReceiverSnafu {
                        completed: this.completed,
                    }
                    .into_error(source))));
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

/// A host failure while yielding chunks; EOF is successful completion.
#[derive(Debug, Snafu)]
#[snafu(display("read failed after {completed} bytes"), visibility(pub(crate)))]
pub struct ReadChunksError<E> {
    pub source: E,
    pub completed: usize,
}

impl<E> ReadChunksError<E> {
    /// Bytes yielded before the host failure.
    pub fn completed(&self) -> usize {
        self.completed
    }

    /// The original host error. Every chunk failure has one.
    pub fn host_error(&self) -> &E {
        &self.source
    }
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
impl<H: ChunkHost> CallOn<H> for ReadChunks {
    type Yield = H::Chunk;
    type Return = Result<usize, ReadChunksError<H::Error>>;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return>
    where
        H: 'visit,
    {
        let this = self.get_mut();
        let Some(maximum) =
            NonZeroUsize::new((this.maximum - this.completed).min(this.chunk_maximum.get()))
        else {
            return Poll::Ready(Complete(Ok(this.completed)));
        };
        let host = ready!(host.poll_view(cx));
        match ready!(H::poll_read_chunk(host, cx, maximum)) {
            Ok(Some(chunk)) => {
                let count = chunk.remaining();
                assert!(count > 0 && count <= maximum.get(), "invalid chunk length");
                this.completed += count;
                Poll::Ready(Yielded(chunk))
            }
            Ok(None) => Poll::Ready(Complete(Ok(this.completed))),
            Err(source) => Poll::Ready(Complete(Err(ReadChunksSnafu {
                completed: this.completed,
            }
            .into_error(source)))),
        }
    }
}

/// Yield exactly `length` bytes, reporting premature EOF with prior progress.
pub struct ReadChunksExact(ReadChunks);

/// Failures that [`ReadChunksExact`](crate::calls::ReadChunksExact) can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_chunks_exact), visibility(pub(crate)))]
pub enum ReadChunksExactError<E> {
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadChunksExactError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::Incomplete { .. } => None,
        }
    }
}
impl ReadChunksExact {
    pub fn new(length: usize, chunk_maximum: NonZeroUsize) -> Self {
        Self(ReadChunks::new(length, chunk_maximum))
    }
}
impl<H: ChunkHost> CallOn<H> for ReadChunksExact {
    type Yield = H::Chunk;
    type Return = Result<(), ReadChunksExactError<H::Error>>;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return>
    where
        H: 'visit,
    {
        let this = self.get_mut();
        CallOn::<H>::poll_call(Pin::new(&mut this.0), host, cx).map(|state| match state {
            Yielded(chunk) => Yielded(chunk),
            Complete(Err(error)) => Complete(Err(read_chunks_exact::ReceiverSnafu {
                completed: error.completed,
            }
            .into_error(error.source))),
            Complete(Ok(completed)) if completed == this.0.maximum => Complete(Ok(())),
            Complete(Ok(completed)) => Complete(Err(error::IncompleteSnafu {
                expected: this.0.maximum,
                completed,
            }
            .build()
            .into())),
        })
    }
}

/// Read a fixed-size array without selecting a chunk representation.
pub struct ReadArray<const LENGTH: usize> {
    bytes: [u8; LENGTH],
    completed: usize,
}

/// Failures that [`ReadArray`](crate::calls::ReadArray) can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_array), visibility(pub(crate)))]
pub enum ReadArrayError<E> {
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadArrayError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::Incomplete { .. } => None,
        }
    }
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
impl<H: ReadHost, const LENGTH: usize> CallOn<H> for ReadArray<LENGTH> {
    type Yield = Infallible;
    type Return = Result<[u8; LENGTH], ReadArrayError<H::Error>>;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Step<Self::Yield, Self::Return>
    where
        H: 'visit,
    {
        let this = self.get_mut();
        if this.completed == LENGTH {
            return Poll::Ready(Complete(Ok(this.bytes)));
        }
        let host = ready!(host.poll_view(cx));
        let mut destination = &mut this.bytes[this.completed..];
        let result = ready!(poll_exact::<H, _>(
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

/// Failures that [`ReadLe`](crate::calls::ReadLe) can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_le), visibility(pub(crate)))]
pub enum ReadLeError<E> {
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadLeError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::Incomplete { .. } => None,
        }
    }
}

impl<E> From<ReadArrayError<E>> for ReadLeError<E> {
    fn from(error: ReadArrayError<E>) -> Self {
        match error {
            ReadArrayError::Incomplete { source } => source.into(),
            ReadArrayError::Receiver { source, completed } => {
                read_le::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}
/// Decode a primitive number in big endian after reading its complete array.
pub struct ReadBe<T: EndianValue> {
    array: T::ArrayCall,
    marker: PhantomData<fn() -> T>,
}

/// Failures that [`ReadBe`](crate::calls::ReadBe) can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_be), visibility(pub(crate)))]
pub enum ReadBeError<E> {
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadBeError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::Incomplete { .. } => None,
        }
    }
}

impl<E> From<ReadArrayError<E>> for ReadBeError<E> {
    fn from(error: ReadArrayError<E>) -> Self {
        match error {
            ReadArrayError::Incomplete { source } => source.into(),
            ReadArrayError::Receiver { source, completed } => {
                read_be::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
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
        endian_impl!(@call ReadLe, ReadLeError, read_le, $number);
        endian_impl!(@call ReadBe, ReadBeError, read_be, $number);
    )*};
    (@call $name:ident, $error:ident, $method:ident, $number:ty) => {
        impl<H: ReadHost> CallOn<H> for $name<$number> {
            type Yield = Infallible;
            type Return = Result<$number, $error<H::Error>>;
            fn poll_call<'visit>(self: Pin<&mut Self>, host: Pin<&mut dyn nitori_call::ReceiverScope<'visit, Family=H>>, cx: &mut Context<'_>) -> Step<Self::Yield, Self::Return> where H: 'visit {
                use std::io::Read as _;
                CallOn::<H>::poll_call(Pin::new(&mut self.get_mut().array), host, cx).map(|state| match state {
                    Yielded(never) => match never {},
                    Complete(result) => Complete(result.map(|bytes| bytes.as_slice().$method::<$number>().expect("complete numeric array")).map_err($error::from)),
                })
            }
        }
    };
}
endian_impl!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);

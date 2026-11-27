#![doc = include_str!("../README.md")]
#![cfg_attr(not(test), no_std)]
#![feature(impl_trait_in_assoc_type, associated_type_defaults)]
#![forbid(unsafe_code)]

use core::future::Future;
use nitori_call::{AccessMode, OnAccess, ReceiverAccess};
use staged::AttachIo;

pub mod host;
mod impls;
pub mod option;
pub mod payload;
pub mod prelude;
mod scoped;
pub mod staged;
mod value;

/// Decode a value on an access borrowed for the decoding: what a value
/// implements to be decoded.
///
/// Implement it for every access that can provide the value's bytes,
/// usually `S: ReceiverAccess` with a bound on the family. The framework
/// gives [`DecodeFrom`] for it: it tells the access where the value starts
/// and whether it was finished or abandoned ([`TrackValues`]), so read the
/// value's fields through [`Decode::decode`] or their [`DecodeFrom`], not
/// through their `decode_on`.
///
/// A value that keeps the source, decoded in stages, reads what comes first
/// and returns [`KeepsIo<Self>`](staged::KeepsIo) as [`Decoded`](Self::Decoded):
/// the framework puts the source in once the value is finished.
///
/// [`TrackValues`]: nitori_call::TrackValues
pub trait DecodeOn<S>: Sized {
    /// Why decoding failed.
    type Error;
    /// What decoding on the borrowed access gives: the value, or for a value
    /// that keeps the source, the value waiting for it
    /// ([`KeepsIo<Self>`](staged::KeepsIo)).
    type Decoded: AttachIo<S, Self> = Self;
    /// Read the value from `io`.
    fn decode_on(io: &mut S) -> impl Future<Output = Result<Self::Decoded, Self::Error>>;
}

/// Encode a value on an access borrowed for the encoding: what a value
/// implements to be encoded.
///
/// As for [`DecodeOn`], the framework gives [`EncodeInto`] for it and
/// tells the access where the value starts and ends. The value chooses its
/// output: usually the sink's reports of the bytes written, merged; for a
/// source written out as a payload, also the count and the merged metadata
/// of what was read; for a value written in stages, a value that keeps the
/// sink to write the rest, returned as [`KeepsIo<Self::Output>`](staged::KeepsIo)
/// in [`Encoded`](Self::Encoded).
pub trait EncodeOn<S> {
    /// What encoding produced.
    type Output;
    /// Why encoding failed.
    type Error;
    /// What encoding on the borrowed access gives: the output, or for an
    /// output that keeps the sink, the output waiting for it.
    type Encoded: AttachIo<S, Self::Output> = Self::Output;
    /// Write the value into `io`.
    fn encode_on(self, io: &mut S) -> impl Future<Output = Result<Self::Encoded, Self::Error>>;
}

/// Decode a value from a source of type `S`: the access by value or as
/// `&mut` ([`OnAccess`]), or a receiver held by value ([`OnReceiver`](nitori_call::OnReceiver)),
/// which decodes in a call on it a value that keeps the receiver.
///
/// The framework implements it for every [`DecodeOn`] value; values do not
/// implement it themselves.
pub trait DecodeFrom<S, How: AccessMode = OnAccess>: Sized {
    /// Why decoding failed.
    type Error;
    /// Read the value from `source`.
    fn decode_from(source: S) -> impl Future<Output = Result<Self, Self::Error>>;
    #[doc(hidden)]
    fn __framework(_: scoped::Framework);
}

/// Encode a value into a sink of type `S`, as [`DecodeFrom`] decodes.
///
/// The framework implements it for every [`EncodeOn`] value; values do not
/// implement it themselves.
pub trait EncodeInto<S, How: AccessMode = OnAccess> {
    /// What encoding produced.
    type Output;
    /// Why encoding failed.
    type Error;
    /// Write the value into `sink`.
    fn encode_into(self, sink: S) -> impl Future<Output = Result<Self::Output, Self::Error>>;
    #[doc(hidden)]
    fn __framework(_: scoped::Framework);
}

/// The number of bytes a value encodes to.
///
/// Telling it can wait, as for a value whose bytes come from a source that
/// learns its length later, and can fail, as for such a source after a
/// reset; `&mut self` lets such a value query its source. A value that
/// always knows its length at once also implements [`KnownLen`].
pub trait EncodedLen {
    /// The type of the length: `usize` for a value in memory, a wider type
    /// where a length can exceed `usize`, such as `u64` for QUIC lengths on
    /// a 32-bit target. A value made of fields chooses its own and widens
    /// the fields' lengths into it with [`Widen`].
    type Length;
    /// Why the length cannot be known; `!` for a value that always knows
    /// it.
    type LengthError;
    /// Return the encoded byte length.
    fn encoded_len(&mut self) -> impl Future<Output = Result<Self::Length, Self::LengthError>>;
}

/// The number of bytes a value encodes to, known at once: what a value in
/// memory, such as a byte array or a literal, tells without waiting.
///
/// A value that wraps a source, such as [`payload::Chunks`], implements
/// only [`EncodedLen`]. A constructor that needs a length comes in a pair:
/// the synchronous one under the plain name takes a `KnownLen` value, the
/// one suffixed `_async` takes an [`EncodedLen`] value.
pub trait KnownLen: EncodedLen<LengthError = !> {
    /// Return the encoded byte length.
    fn known_len(&self) -> Self::Length;
}

/// Convert a length into a wider integer type without losing its value,
/// to add the lengths of fields of different types.
///
/// Implemented from each unsigned integer type into itself and into every
/// wider one, `usize` included where the target's pointer width makes the
/// conversion lossless; the standard library has no `From<usize> for u64`.
pub trait Widen<T> {
    /// The same value as a `T`.
    fn widen(self) -> T;
}

impl<T> Widen<T> for T {
    fn widen(self) -> T {
        self
    }
}

macro_rules! widen {
    ($($from:ty => $($to:ty),+;)+) => {$($(
        impl Widen<$to> for $from {
            fn widen(self) -> $to {
                <$to>::from(self)
            }
        }
    )+)+};
}
widen! {
    u8 => u16, u32, u64, u128, usize;
    u16 => u32, u64, u128, usize;
    u32 => u64, u128;
    u64 => u128;
}

/// A `usize` widened by the pointer width the target has.
macro_rules! widen_usize {
    ($($width:literal: $($from:ty => $to:ty),+;)+) => {$($(
        #[cfg(target_pointer_width = $width)]
        impl Widen<$to> for $from {
            fn widen(self) -> $to {
                // invariant: the pointer width checked above makes the
                // conversion lossless.
                self as $to
            }
        }
    )+)+};
}
widen_usize! {
    "16": usize => u16, usize => u32, usize => u64, usize => u128;
    "32": u32 => usize, usize => u32, usize => u64, usize => u128;
    "64": u32 => usize, u64 => usize, usize => u64, usize => u128;
}

/// Decode values from this access inside an operation: `io.decode::<V>()`.
pub trait Decode: ReceiverAccess {
    /// Decode a `V` from this access, borrowed for the decoding.
    fn decode<'a, V: DecodeFrom<&'a mut Self>>(
        &'a mut self,
    ) -> impl Future<Output = Result<V, V::Error>>
    where
        Self: Sized,
    {
        V::decode_from(self)
    }
}
impl<A: ReceiverAccess + ?Sized> Decode for A {}

/// Encode values into this access inside an operation: `io.encode(value)`.
pub trait Encode: ReceiverAccess {
    /// Encode `value` into this access, borrowed for the encoding.
    fn encode<'a, V: EncodeInto<&'a mut Self>>(
        &'a mut self,
        value: V,
    ) -> impl Future<Output = Result<V::Output, V::Error>>
    where
        Self: Sized,
    {
        value.encode_into(self)
    }
}
impl<A: ReceiverAccess + ?Sized> Encode for A {}

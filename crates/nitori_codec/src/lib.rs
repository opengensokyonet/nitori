#![doc = include_str!("../README.md")]
#![cfg_attr(not(test), no_std)]
#![feature(impl_trait_in_assoc_type)]
#![forbid(unsafe_code)]

use core::future::Future;
use nitori_call::ReceiverAccess;

pub mod host;
mod impls;
pub mod payload;
pub mod prelude;
mod value;

/// Decode a value from a source of type `S`.
///
/// `S` is the source by value or as `&mut`. Implement it on the value: for
/// every source that can provide its bytes, usually `S: AsAccess` with a
/// bound on the family, or for a source type of the value's own crate. A
/// value that keeps `S` is decoded in stages: it reads what comes first
/// and leaves the rest in the source it keeps.
pub trait DecodeFrom<S>: Sized {
    /// Why decoding failed.
    type Error;
    /// Read the value from `source`.
    fn decode_from(source: S) -> impl Future<Output = Result<Self, Self::Error>>;
}

/// Encode a value into a sink of type `S`.
///
/// `S` is the sink by value or as `&mut`, as for [`DecodeFrom`]. The value
/// chooses its output: usually the sink's reports of the bytes written,
/// merged; for a source written out as a payload, also the count and the
/// merged metadata of what was read; for a value written in stages, a value
/// that keeps the sink to write the rest.
pub trait EncodeInto<S> {
    /// What encoding produced.
    type Output;
    /// Why encoding failed.
    type Error;
    /// Write the value into `sink`.
    fn encode_into(self, sink: S) -> impl Future<Output = Result<Self::Output, Self::Error>>;
}

/// The number of bytes a value encodes to.
///
/// Telling it can wait, as for a value whose bytes come from a source that
/// learns its length later, and can fail, as for such a source after a
/// reset; `&mut self` lets such a value query its source.
pub trait EncodedLen {
    /// Why the length cannot be known; `!` for a value that always knows
    /// it.
    type LengthError;
    /// Return the encoded byte length.
    fn encoded_len(&mut self) -> impl Future<Output = Result<u64, Self::LengthError>>;
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

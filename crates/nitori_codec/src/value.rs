//! Values with the merged metadata of their bytes when read, or the write
//! options of their bytes when written.
use crate::{DecodeFrom, DecodeOn, EncodeInto, EncodeOn, EncodedLen, KnownLen};
use nitori_call::ReceiverAccess;
use nitori_io::{
    Combine, Read, Value, Write,
    adapters::{FixedOptions, MappedOptions, MergedMetadata},
};

/// Decodes `V` from a source with metadata and returns the merged metadata
/// of the bytes it read: `V` reads from a [`MergedMetadata`], which reports
/// no metadata to it and merges the source's.
///
/// This is how a base value, one that reads its own bytes and accepts only
/// sources without metadata, is read from a source with it; a value that
/// holds nothing of its own, such as a constant marker, counts as one. A composite
/// value takes the metadata type as its own parameter and holds each base
/// field as `Value<Field, M>`; a composite field takes the same `M` and
/// is not wrapped.
///
/// A failure or cancellation does not return the metadata of the bytes read
/// before it, as a failed read of an array does not return its bytes.
impl<S, V, M, E> DecodeOn<S> for Value<V, M>
where
    S: ReceiverAccess<Family: Read<Metadata = M>>,
    M: Combine,
    V: for<'x> DecodeFrom<MergedMetadata<&'x mut S, M, &'x mut M>, Error = E>,
{
    type Error = E;
    async fn decode_on(source: &mut S) -> Result<Self, E> {
        let mut merged = M::default();
        let value = V::decode_from(MergedMetadata::borrowing(source, &mut merged)).await?;
        Ok(Value {
            value,
            metadata: merged,
        })
    }
}

/// Encodes `V` into a sink of [`Write<O>`](Write) with `metadata` as the
/// write options of every byte it writes. [`MappedOptions`] with
/// [`FixedOptions`] accepts any options from `V` and passes these to the sink.
///
/// This is how a base value, one that writes its own bytes with the
/// options `()`, is written with options; a composite value takes the
/// options type as its own parameter, as for decoding, and holds each base
/// field as `Value<Field, O>`, so each field's bytes get the field's
/// options. A value decoded from a source with metadata `M` is written
/// into a sink that takes other options through [`MappedOptions`] with a
/// [`MapOptions`](nitori_io::adapters::MapOptions) mapping.
impl<S, V, O, R, E> EncodeOn<S> for Value<V, O>
where
    S: ReceiverAccess<Family: Write<O>>,
    V: for<'x> EncodeInto<MappedOptions<&'x mut S, FixedOptions<O>>, Output = R, Error = E>,
{
    type Output = R;
    type Error = E;
    async fn encode_on(self, sink: &mut S) -> Result<R, E> {
        let Value { value, metadata } = self;
        value
            .encode_into(MappedOptions::new(sink, FixedOptions(metadata)))
            .await
    }
}

/// The value's encoded length.
impl<V: EncodedLen, M> EncodedLen for Value<V, M> {
    type Length = V::Length;
    type LengthError = V::LengthError;
    async fn encoded_len(&mut self) -> Result<V::Length, V::LengthError> {
        self.value.encoded_len().await
    }
}

/// The value's encoded length, known at once when the value knows it.
impl<V: KnownLen, M> KnownLen for Value<V, M> {
    fn known_len(&self) -> V::Length {
        self.value.known_len()
    }
}

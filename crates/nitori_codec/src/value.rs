//! Values with the merged metadata of their bytes when read, or the write
//! options of their bytes when written.
use crate::{DecodeFrom, EncodeInto, EncodedLen};
use nitori_call::{AsAccess, Composed};
use nitori_io::{
    Combine, Read, Value, Write,
    adapters::{FixedOptions, MappedOptions, MergedMetadata, MergingSource},
};

/// Decodes `V` from a source with metadata and returns the merged metadata
/// of the bytes it read: `V` reads from a [`MergingSource`], which reports
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
impl<S, V, M, E> DecodeFrom<S> for Value<V, M>
where
    S: AsAccess<Family: Read<Metadata = M>>,
    M: Combine + Unpin,
    V: for<'x> DecodeFrom<MergingSource<'x, S, M>, Error = E>,
{
    type Error = E;
    async fn decode_from(mut source: S) -> Result<Self, E> {
        let mut merged = MergedMetadata(M::default());
        let value = V::decode_from(MergingSource::new(&mut source, &mut merged)).await?;
        Ok(Value {
            value,
            metadata: merged.0,
        })
    }
}

/// Encodes `V` into a sink of [`Write<O>`](Write) with `metadata` as the
/// write options of every byte it writes: `V` writes to the sink composed
/// with [`MappedOptions`]`(`[`FixedOptions`]`(metadata))`, which takes any
/// options, `()` included, and gives the sink these. That composition
/// state must be `Unpin`, so `V`'s bound holds only for `O: Unpin`.
///
/// This is how a base value, one that writes its own bytes with the
/// options `()`, is written with options; a composite value takes the
/// options type as its own parameter, as for decoding, and holds each base
/// field as `Value<Field, O>`, so each field's bytes get the field's
/// options. A value decoded from a source with metadata `M` is written
/// into a sink that takes other options through [`MappedOptions`] with a
/// [`MapOptions`](nitori_io::adapters::MapOptions) mapping.
impl<S, V, O, R, E> EncodeInto<S> for Value<V, O>
where
    S: AsAccess<Family: Write<O>>,
    V: for<'x> EncodeInto<
            Composed<&'x mut S::Access, MappedOptions<FixedOptions<O>>>,
            Output = R,
            Error = E,
        >,
{
    type Output = R;
    type Error = E;
    async fn encode_into(self, mut sink: S) -> Result<R, E> {
        let Value { value, metadata } = self;
        value
            .encode_into(Composed::new(
                sink.access(),
                MappedOptions(FixedOptions(metadata)),
            ))
            .await
    }
}

/// The value's encoded length.
impl<V: EncodedLen, M> EncodedLen for Value<V, M> {
    type LengthError = V::LengthError;
    async fn encoded_len(&mut self) -> Result<u64, V::LengthError> {
        self.value.encoded_len().await
    }
}

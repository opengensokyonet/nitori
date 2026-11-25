//! Values with the merged metadata of their bytes.
use crate::{DecodeFrom, EncodeInto, EncodedLen};
use nitori_call::AsAccess;
use nitori_io::{
    Combine, Read,
    adapters::{MergedMetadata, MergingSource},
    calls::ReadValue,
};

/// Decodes `V` from a source with metadata and returns the merged metadata
/// of the bytes it read: `V` reads from a [`MergingSource`], which reports
/// no metadata to it and merges the source's.
///
/// This is how a base value, one that reads its own bytes and accepts only
/// sources without metadata, is read from a source with it; a value that
/// holds nothing of its own, such as a constant marker, counts as one. A composite
/// value takes the metadata type as its own parameter and holds each base
/// field as `ReadValue<Field, M>`; a composite field takes the same `M` and
/// is not wrapped.
///
/// A failure or cancellation does not return the metadata of the bytes read
/// before it, as a failed read of an array does not return its bytes.
impl<S, V, M, E> DecodeFrom<S> for ReadValue<V, M>
where
    S: AsAccess<Family: Read<Metadata = M>>,
    M: Combine + Unpin,
    V: for<'x> DecodeFrom<MergingSource<'x, S, M>, Error = E>,
{
    type Error = E;
    async fn decode_from(mut source: S) -> Result<Self, E> {
        let mut merged = MergedMetadata(M::default());
        let value = V::decode_from(MergingSource::new(&mut source, &mut merged)).await?;
        Ok(ReadValue {
            value,
            metadata: merged.0,
        })
    }
}

/// Encodes the value; metadata describes bytes that were read, so it is not
/// written, whatever its type.
impl<S, V: EncodeInto<S>, M> EncodeInto<S> for ReadValue<V, M> {
    type Output = V::Output;
    type Error = V::Error;
    async fn encode_into(self, sink: S) -> Result<V::Output, V::Error> {
        self.value.encode_into(sink).await
    }
}

/// The value's encoded length.
impl<V: EncodedLen, M> EncodedLen for ReadValue<V, M> {
    type LengthError = V::LengthError;
    async fn encoded_len(&mut self) -> Result<u64, V::LengthError> {
        self.value.encoded_len().await
    }
}

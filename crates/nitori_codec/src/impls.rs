//! The byte values: arrays, slices and `Bytes`.
use crate::{DecodeFrom, EncodeInto, EncodedLen};
use bytes::Bytes;
use nitori_call::AsAccess;
use nitori_io::{
    Combine, Read, Write,
    calls::{
        ReadArrayError, ReadArrayExt as _, WriteAllError, WriteAllExt as _, WriteArrayError,
        WriteArrayExt as _,
    },
};

/// A byte count as the length of an encoding.
pub(crate) fn encoded_length(count: usize) -> u64 {
    // invariant: a usize fits in a u64 on every supported target. A source's
    // generic length can be wider, which payload::SourceLengthError reports.
    u64::try_from(count).expect("a byte count fits in 64 bits")
}

/// Reads as many bytes as the array holds, from a source without metadata.
impl<S, const N: usize> DecodeFrom<S> for [u8; N]
where
    S: AsAccess<Family: Read<Metadata = ()>>,
{
    type Error = ReadArrayError<<S::Family as Read>::Error>;
    async fn decode_from(mut source: S) -> Result<Self, Self::Error> {
        Ok(source.access().read_array::<N>().await?.value)
    }
}

/// Writes the array whole and returns the merged report.
impl<S, const N: usize> EncodeInto<S> for [u8; N]
where
    S: AsAccess<Family: Write<Report: Combine>>,
{
    type Output = <S::Family as Write>::Report;
    type Error = WriteArrayError<<S::Family as Write>::Error>;
    async fn encode_into(self, mut sink: S) -> Result<Self::Output, Self::Error> {
        sink.access().write_array(self).await
    }
}

/// Writes the slice whole and returns the merged report.
impl<S> EncodeInto<S> for &[u8]
where
    S: AsAccess<Family: Write<Report: Combine>>,
{
    type Output = <S::Family as Write>::Report;
    type Error = WriteAllError<<S::Family as Write>::Error>;
    async fn encode_into(self, mut sink: S) -> Result<Self::Output, Self::Error> {
        sink.access().write_all(self).await
    }
}

/// Writes the bytes whole and returns the merged report.
impl<S> EncodeInto<S> for Bytes
where
    S: AsAccess<Family: Write<Report: Combine>>,
{
    type Output = <S::Family as Write>::Report;
    type Error = WriteAllError<<S::Family as Write>::Error>;
    async fn encode_into(self, mut sink: S) -> Result<Self::Output, Self::Error> {
        sink.access().write_all(self).await
    }
}

impl<const N: usize> EncodedLen for [u8; N] {
    type LengthError = !;
    async fn encoded_len(&mut self) -> Result<u64, !> {
        Ok(encoded_length(N))
    }
}
impl EncodedLen for &[u8] {
    type LengthError = !;
    async fn encoded_len(&mut self) -> Result<u64, !> {
        Ok(encoded_length(self.len()))
    }
}
impl EncodedLen for Bytes {
    type LengthError = !;
    async fn encoded_len(&mut self) -> Result<u64, !> {
        Ok(encoded_length(self.len()))
    }
}

//! The byte values: arrays, slices and `Bytes`.
use crate::{DecodeOn, EncodeOn, EncodedLen, KnownLen};
use bytes::Bytes;
use nitori_call::ReceiverAccess;
use nitori_io::{
    Combine, Read, Write,
    calls::{
        ReadArrayError, ReadArrayExt as _, WriteAllError, WriteAllExt as _, WriteArrayError,
        WriteArrayExt as _,
    },
};

/// Reads as many bytes as the array holds, from a source without metadata.
impl<S, const N: usize> DecodeOn<S> for [u8; N]
where
    S: ReceiverAccess<Family: Read<Metadata = ()>>,
{
    type Error = ReadArrayError<<S::Family as Read>::Error>;
    async fn decode_on(source: &mut S) -> Result<Self, Self::Error> {
        Ok(source.read_array::<N>().await?.value)
    }
}

/// Writes the array whole with the write options `()` and
/// returns the merged report.
impl<S, const N: usize> EncodeOn<S> for [u8; N]
where
    S: ReceiverAccess<Family: Write<(), Report: Combine>>,
{
    type Output = <S::Family as Write<()>>::Report;
    type Error = WriteArrayError<<S::Family as Write<()>>::Error>;
    async fn encode_on(self, sink: &mut S) -> Result<Self::Output, Self::Error> {
        sink.write_array(self, ()).await
    }
}

/// Writes the slice whole with the write options `()` and
/// returns the merged report.
impl<S> EncodeOn<S> for &[u8]
where
    S: ReceiverAccess<Family: Write<(), Report: Combine>>,
{
    type Output = <S::Family as Write<()>>::Report;
    type Error = WriteAllError<<S::Family as Write<()>>::Error>;
    async fn encode_on(self, sink: &mut S) -> Result<Self::Output, Self::Error> {
        sink.write_all(self, ()).await
    }
}

/// Writes the bytes whole with the write options `()` and
/// returns the merged report.
impl<S> EncodeOn<S> for Bytes
where
    S: ReceiverAccess<Family: Write<(), Report: Combine>>,
{
    type Output = <S::Family as Write<()>>::Report;
    type Error = WriteAllError<<S::Family as Write<()>>::Error>;
    async fn encode_on(self, sink: &mut S) -> Result<Self::Output, Self::Error> {
        sink.write_all(self, ()).await
    }
}

/// Byte values in memory know their length at once.
macro_rules! known_len {
    ($([$($g:tt)*] $ty:ty => |$this:ident| $len:expr;)+) => {$(
        impl<$($g)*> EncodedLen for $ty {
            type Length = usize;
            type LengthError = !;
            async fn encoded_len(&mut self) -> Result<usize, !> {
                Ok(self.known_len())
            }
        }
        impl<$($g)*> KnownLen for $ty {
            fn known_len(&self) -> usize {
                let $this = self;
                $len
            }
        }
    )+};
}
known_len! {
    [const N: usize] [u8; N] => |_this| N;
    [] &[u8] => |this| this.len();
    [] Bytes => |this| this.len();
}

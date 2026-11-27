//! Arrays, slices and `Bytes` as values: decoded and encoded inside an
//! operation and as the whole operation of a call.
#![feature(impl_trait_in_assoc_type)]
use bytes::{Buf, Bytes};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{ReceiverAccess, call};
use nitori_codec::prelude::*;
use nitori_io::{
    Combine, Read, Write, WriteSegment,
    adapters::WriteLimit,
    calls::{ReadArrayError, WriteAllError, WriteArrayError},
};

#[call(sync)]
async fn two_arrays<F: Read<Metadata = ()>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<([u8; 2], [u8; 1]), ReadArrayError<F::Error>> {
    let first = io.decode::<[u8; 2]>().await?;
    Ok((first, io.decode().await?))
}

#[test]
fn arrays_decode_inside_an_operation_and_on_a_host() {
    let mut input = &b"abcde"[..];
    assert_eq!(input.sync_two_arrays_unpin().unwrap(), (*b"ab", *b"c"));
    assert_eq!(input.sync_decode_unpin::<[u8; 2]>().unwrap(), *b"de");
    assert!(input.is_empty());
    let mut shared = Bytes::from_static(b"xyz");
    assert_eq!(shared.sync_decode_unpin::<[u8; 3]>().unwrap(), *b"xyz");
}

#[test]
fn a_short_source_fails_an_array_with_what_it_read() {
    let mut input = &b"a"[..];
    let error = input.sync_decode_unpin::<[u8; 3]>().unwrap_err();
    assert!(
        matches!(error, ReadArrayError::Incomplete { .. }),
        "{error:?}"
    );
    assert_eq!(error.completed(), 1);
}

#[call(sync)]
async fn write_three<F: Write<(), Report: Combine>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<F::Report, WriteAllError<F::Error>>
where
    <F as Write<()>>::Error: core::fmt::Debug,
{
    io.encode(*b"ab").await.unwrap();
    io.encode(&b"cd"[..]).await?;
    io.encode(Bytes::from_static(b"ef")).await
}

#[test]
fn byte_values_encode_inside_an_operation_and_on_a_host() {
    let mut output = Vec::new();
    output.sync_write_three_unpin().unwrap();
    output.sync_encode_unpin(*b"gh").unwrap();
    output.sync_encode_unpin(&b"i"[..]).unwrap();
    output.sync_encode_unpin(Bytes::from_static(b"j")).unwrap();
    assert_eq!(output, b"abcdefghij");
}

#[call(sync)]
async fn bounded<F: Write<(), Report = ()>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<(), WriteArrayError<F::Error>> {
    WriteLimit::new(&mut *io, 2).encode(*b"abc").await
}

#[test]
fn a_full_sink_fails_an_array_with_what_it_accepted() {
    let mut output = Vec::new();
    let error = output.sync_bounded_unpin().unwrap_err();
    assert!(
        matches!(error, WriteArrayError::WriteZero { .. }),
        "{error:?}"
    );
    assert_eq!(error.completed(), 2);
    assert_eq!(output, b"ab");
}

/// How many segments a sink accepted, merged by adding.
#[derive(Debug, Default, PartialEq, Eq)]
struct Segments(usize);
impl Combine for Segments {
    fn combine(self, next: Self) -> Self {
        Self(self.0 + next.0)
    }
}

/// A sink that accepts at most two bytes at a time and reports each
/// acceptance as one segment.
#[derive(Default)]
struct Pairs(Vec<u8>);
nitori_call::family_receiver!(impl [] for Pairs);
impl Write<()> for Pairs {
    type Error = !;
    type Report = Segments;
    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut B,
        _: &(),
    ) -> Poll<Result<Option<WriteSegment<Segments>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let count = input.remaining().min(2);
        this.0.extend_from_slice(&input.chunk()[..count]);
        input.advance(count);
        Poll::Ready(Ok(NonZeroUsize::new(count).map(|count| WriteSegment {
            count,
            report: Segments(1),
        })))
    }
}

#[test]
fn encoding_returns_the_merged_report_of_the_segments() {
    let mut sink = Pairs::default();
    assert_eq!(sink.sync_encode_unpin(*b"abcde").unwrap(), Segments(3));
    assert_eq!(sink.sync_encode_unpin(&b"fg"[..]).unwrap(), Segments(1));
    assert_eq!(sink.sync_encode_unpin(Bytes::new()).unwrap(), Segments(0));
    assert_eq!(sink.0, b"abcdefg");
}

#[test]
fn byte_values_know_their_encoded_length() {
    fn length(mut value: impl EncodedLen<LengthError = !>) -> u64 {
        let Ok(length) = nitori_call::__private::run_sync(value.encoded_len());
        length
    }
    assert_eq!(length([0u8; 4]), 4);
    assert_eq!(length(&b"abc"[..]), 3);
    assert_eq!(length(Bytes::from_static(b"ab")), 2);
    assert_eq!(length([0u8; 0]), 0);
}

/// A source that never has bytes yet.
struct Never;
nitori_call::family_receiver!(impl [] for Never);
impl Read for Never {
    type Error = !;
    type Metadata = ();
    fn poll_read<'v, O: bytes::BufMut + ?Sized>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        _: &mut O,
    ) -> Poll<Result<Option<nitori_io::ReadSegment<()>>, !>>
    where
        Self: 'v,
    {
        Poll::Pending
    }
}

#[test]
#[should_panic(expected = "returned Pending")]
fn a_synchronous_decoding_that_waits_panics() {
    let _ = Never.sync_decode_unpin::<[u8; 1]>();
}

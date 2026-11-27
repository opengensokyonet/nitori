//! Sources written out as values: the three pairings, the merged metadata
//! and reports they return, and the length they report.
#![feature(impl_trait_in_assoc_type)]
use bytes::{BufMut, Bytes};
use core::{
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{ReceiverAccess, call};
use nitori_codec::{
    payload::{Buffered, Chunks, Fill, SourceLengthError},
    prelude::*,
};
use nitori_io::{
    Combine, ExactLen, Read, ReadChunk, ReadSegment, Write, WriteLent,
    adapters::{FixedOptions, MappedOptions, ReadLimit},
    calls::{CopyBufferedError, CopyTotal},
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

#[call(sync)]
async fn write_chunks<F, G>(
    source: &mut impl ReceiverAccess<Family = F>,
    sink: &mut impl ReceiverAccess<Family = G>,
    maximum: NonZeroUsize,
) -> u64
where
    F: ReadChunk<Metadata = (), Error = !>,
    G: Write<(), Report = (), Error = !>,
{
    sink.encode(Chunks {
        source,
        chunk_maximum: maximum,
    })
    .await
    .unwrap()
    .count
}

#[call(sync)]
async fn write_fill<F, G>(
    source: &mut impl ReceiverAccess<Family = F>,
    sink: &mut impl ReceiverAccess<Family = G>,
) -> CopyTotal<F::Metadata, ()>
where
    F: Read<Metadata: Clone + Combine, Error = !>,
    G: WriteLent<F::Metadata, Report = (), Error = !>,
{
    sink.encode(Fill {
        source,
        chunk_maximum: nz(3),
    })
    .await
    .unwrap()
}

#[call(sync)]
async fn write_buffered<F, G>(
    source: &mut impl ReceiverAccess<Family = F>,
    sink: &mut impl ReceiverAccess<Family = G>,
    buffer: &mut [u8],
) -> Result<CopyTotal<F::Metadata, ()>, CopyBufferedError<!, !>>
where
    F: Read<Metadata: Combine, Error = !>,
    G: Write<F::Metadata, Report = (), Error = !>,
{
    sink.encode(Buffered { source, buffer }).await
}

#[test]
fn each_pairing_writes_the_whole_source() {
    let mut output = Vec::new();
    let mut chunks = Bytes::from_static(b"abc");
    assert_eq!((&mut chunks, &mut output).sync_write_chunks_unpin(nz(2)), 3);
    let mut fill = &b"defg"[..];
    assert_eq!((&mut fill, &mut output).sync_write_fill_unpin().count, 4);
    let mut buffered = &b"hi"[..];
    assert_eq!(
        (&mut buffered, &mut output)
            .sync_write_buffered_unpin(&mut [0; 1])
            .unwrap()
            .count,
        2
    );
    assert_eq!(output, b"abcdefghi");
}

#[call(sync)]
async fn write_bounded<F, G>(
    source: &mut impl ReceiverAccess<Family = F>,
    sink: &mut impl ReceiverAccess<Family = G>,
) -> u64
where
    F: ReadChunk<Metadata = (), Error = !> + ExactLen<Length = usize, LengthError = !>,
    G: Write<(), Report = (), Error = !>,
{
    let mut payload = Chunks {
        source: ReadLimit::new(&mut *source, 4u8),
        chunk_maximum: nz(8),
    };
    assert_eq!(payload.encoded_len().await.unwrap(), 4);
    sink.encode(payload).await.unwrap().count
}

#[test]
fn a_bounded_source_writes_only_its_bound() {
    let mut source = &b"abcdef"[..];
    let mut output = Vec::new();
    assert_eq!((&mut source, &mut output).sync_write_bounded_unpin(), 4);
    assert_eq!(output, b"abcd");
    assert_eq!(source, b"ef");
}

#[test]
fn an_empty_buffer_cannot_carry_a_source() {
    let mut source = &b"a"[..];
    let mut output = Vec::new();
    let error = (&mut source, &mut output)
        .sync_write_buffered_unpin(&mut [])
        .unwrap_err();
    assert!(
        matches!(error, CopyBufferedError::InsufficientCapacity { .. }),
        "{error:?}"
    );
    assert!(output.is_empty());
}

/// Whether any of the bytes arrived in 0-RTT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ZeroRtt(bool);
impl Combine for ZeroRtt {
    fn combine(self, next: Self) -> Self {
        Self(self.0 || next.0)
    }
}

/// A source that reads at most two bytes at a time; the first two bytes
/// arrived in 0-RTT.
struct Received {
    data: &'static [u8],
    read: usize,
}
nitori_call::family_receiver!(impl [] for Received);
impl Read for Received {
    type Error = !;
    type Metadata = ZeroRtt;
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<Option<ReadSegment<ZeroRtt>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let early = this.read < 2;
        let limit = if early { 2 - this.read } else { 2 };
        let count = (this.data.len() - this.read)
            .min(limit)
            .min(out.remaining_mut());
        out.put_slice(&this.data[this.read..this.read + count]);
        this.read += count;
        Poll::Ready(Ok(NonZeroUsize::new(count).map(|count| ReadSegment {
            count,
            metadata: ZeroRtt(early),
        })))
    }
}

#[test]
fn writing_a_source_returns_the_merged_metadata_of_what_it_read() {
    // The vector takes no write options: give it `()` for every segment,
    // whatever it was read with.
    let mut output = MappedOptions::new(Vec::new(), FixedOptions(()));
    let mut source = Received {
        data: b"abcde",
        read: 0,
    };
    let total = (&mut source, &mut output)
        .sync_write_buffered_unpin(&mut [0; 8])
        .unwrap();
    assert_eq!((total.count, total.metadata), (5, ZeroRtt(true)));
    let mut source = Received {
        data: b"abcde",
        read: 2,
    };
    let total = (&mut source, &mut output).sync_write_fill_unpin();
    assert_eq!((total.count, total.metadata), (3, ZeroRtt(false)));
    assert_eq!(output.into_parts().io, b"abcdecde");
}

/// A source whose length is wider than 64 bits.
struct Huge;
nitori_call::family_receiver!(impl [] for Huge);
impl ExactLen for Huge {
    type Length = u128;
    type LengthError = !;
    fn poll_remaining_len<'v>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
    ) -> Poll<Result<u128, !>>
    where
        Self: 'v,
    {
        Poll::Ready(Ok(u128::from(u64::MAX) + 1))
    }
}

/// A source reset before it learned its length.
struct Reset;
#[derive(Debug, PartialEq, Eq)]
struct Reason;
nitori_call::family_receiver!(impl [] for Reset);
impl ExactLen for Reset {
    type Length = u64;
    type LengthError = Reason;
    fn poll_remaining_len<'v>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
    ) -> Poll<Result<u64, Reason>>
    where
        Self: 'v,
    {
        Poll::Ready(Err(Reason))
    }
}

#[call]
async fn chunks_len<F: ExactLen>(
    source: &mut impl ReceiverAccess<Family = F>,
) -> Result<u64, SourceLengthError<F::LengthError>>
where
    u64: TryFrom<<F as ExactLen>::Length>,
{
    Chunks {
        source,
        chunk_maximum: nz(1),
    }
    .encoded_len()
    .await
}

#[call]
async fn fill_len<F: ExactLen>(
    source: &mut impl ReceiverAccess<Family = F>,
) -> Result<u64, SourceLengthError<F::LengthError>>
where
    u64: TryFrom<<F as ExactLen>::Length>,
{
    Fill {
        source,
        chunk_maximum: nz(1),
    }
    .encoded_len()
    .await
}

#[test]
fn a_length_the_source_cannot_tell_or_that_does_not_fit_fails() {
    let too_long = nitori_call::__private::run_sync(Huge.chunks_len_unpin());
    assert!(matches!(too_long, Err(SourceLengthError::TooLong)));
    let unknown = nitori_call::__private::run_sync(Reset.fill_len_unpin());
    assert!(matches!(
        unknown,
        Err(SourceLengthError::Source { source: Reason })
    ));
}

/// A source whose length is known after one Pending.
struct Later {
    polls: usize,
}
nitori_call::family_receiver!(impl [] for Later);
impl ExactLen for Later {
    type Length = usize;
    type LengthError = !;
    fn poll_remaining_len<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<usize, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        this.polls += 1;
        if this.polls == 1 {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        Poll::Ready(Ok(5))
    }
}

#[call]
async fn buffered_len<F: ExactLen<Length = usize, LengthError = !>>(
    source: &mut impl ReceiverAccess<Family = F>,
) -> Result<u64, SourceLengthError<!>> {
    Buffered {
        source,
        buffer: [0u8; 1],
    }
    .encoded_len()
    .await
}

#[test]
fn a_length_waits_for_the_source() {
    let mut receiver = Later { polls: 0 };
    let mut length = core::pin::pin!(receiver.buffered_len_unpin());
    let mut cx = Context::from_waker(core::task::Waker::noop());
    assert!(length.as_mut().poll(&mut cx).is_pending());
    assert!(matches!(length.as_mut().poll(&mut cx), Poll::Ready(Ok(5))));
}

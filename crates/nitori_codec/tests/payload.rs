//! Sources written out as values: the three pairings, the merged metadata
//! and reports they return, and the length they report.
use bytes::{BufMut, Bytes};
use core::{
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{Composed, ComposedReceiver, ConstructedAccess};
use nitori_codec::{
    payload::{Buffered, Chunks, Fill, SourceLengthError},
    prelude::*,
};
use nitori_io::{
    Combine, ExactLen, Read, ReadSegment,
    adapters::{FixedOptions, MappedOptions, ReadLimit},
    calls::{CopyBufferedError, CopyTotal},
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

#[test]
fn each_pairing_writes_the_whole_source() {
    let mut output = Vec::new();
    let total = output
        .sync_encode_unpin(Chunks {
            source: ConstructedAccess::new(Bytes::from_static(b"abc")),
            chunk_maximum: nz(2),
        })
        .unwrap();
    assert_eq!(total.count, 3);
    let total = output
        .sync_encode_unpin(Fill {
            source: ConstructedAccess::new(&b"defg"[..]),
            chunk_maximum: nz(3),
        })
        .unwrap();
    assert_eq!(total.count, 4);
    let total = output
        .sync_encode_unpin(Buffered {
            source: ConstructedAccess::new(&b"hi"[..]),
            buffer: [0u8; 1],
        })
        .unwrap();
    assert_eq!(total.count, 2);
    assert_eq!(output, b"abcdefghi");
}

#[test]
fn a_bounded_source_writes_only_its_bound() {
    let mut source = &b"abcdef"[..];
    let mut output = Vec::new();
    let mut payload = Chunks {
        source: Composed::new(ConstructedAccess::new(&mut source), ReadLimit(4u8)),
        chunk_maximum: nz(8),
    };
    let Ok(length) = nitori_call::__private::run_sync(payload.encoded_len()) else {
        panic!("a memory source knows its length");
    };
    assert_eq!(length, 4);
    assert_eq!(output.sync_encode_unpin(payload).unwrap().count, 4);
    assert_eq!(output, b"abcd");
    assert_eq!(source, b"ef");
}

#[test]
fn an_empty_buffer_cannot_carry_a_source() {
    let mut output = Vec::new();
    let error = output
        .sync_encode_unpin(Buffered {
            source: ConstructedAccess::new(&b"a"[..]),
            buffer: [0u8; 0],
        })
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
    let mut output = ComposedReceiver::new(Vec::new(), MappedOptions(FixedOptions(())));
    let total: CopyTotal<ZeroRtt, ()> = output
        .sync_encode_unpin(Buffered {
            source: ConstructedAccess::new(Received {
                data: b"abcde",
                read: 0,
            }),
            buffer: [0u8; 8],
        })
        .unwrap();
    assert_eq!((total.count, total.metadata), (5, ZeroRtt(true)));
    let total = output
        .sync_encode_unpin(Fill {
            source: ConstructedAccess::new(Received {
                data: b"abcde",
                read: 2,
            }),
            chunk_maximum: nz(8),
        })
        .unwrap();
    assert_eq!((total.count, total.metadata), (3, ZeroRtt(false)));
    assert_eq!(output.into_parts().0, b"abcdecde");
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

#[test]
fn a_length_the_source_cannot_tell_or_that_does_not_fit_fails() {
    let too_long = nitori_call::__private::run_sync(
        Chunks {
            source: ConstructedAccess::new(Huge),
            chunk_maximum: nz(1),
        }
        .encoded_len(),
    );
    assert!(matches!(too_long, Err(SourceLengthError::TooLong)));
    let unknown = nitori_call::__private::run_sync(
        Fill {
            source: ConstructedAccess::new(Reset),
            chunk_maximum: nz(1),
        }
        .encoded_len(),
    );
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

#[test]
fn a_length_waits_for_the_source() {
    let mut payload = Buffered {
        source: ConstructedAccess::new(Later { polls: 0 }),
        buffer: [0u8; 1],
    };
    let mut length = core::pin::pin!(payload.encoded_len());
    let mut cx = Context::from_waker(core::task::Waker::noop());
    assert!(length.as_mut().poll(&mut cx).is_pending());
    assert!(matches!(length.as_mut().poll(&mut cx), Poll::Ready(Ok(5))));
}

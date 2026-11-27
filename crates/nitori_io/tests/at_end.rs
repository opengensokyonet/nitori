#![feature(impl_trait_in_assoc_type)]
//! End queries at the source cursor, through the capabilities and adapters.
use bytes::{Bytes, BytesMut};
use nitori_call::{ReceiverAccess, call, family_receiver};
use nitori_io::{
    adapters::{Chunked, MergedMetadata, ReadChunkSkip, ReadLimit, ReadSkip},
    prelude::*,
};
use std::{
    collections::VecDeque,
    future::Future,
    num::NonZeroUsize,
    pin::{Pin, pin},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

#[call(sync)]
async fn query_end<F: AtEnd>(
    source: &mut impl ReceiverAccess<Family = F>,
) -> Result<bool, F::RemainderError> {
    source.at_end().await
}

#[test]
fn memory_sources_answer_without_taking_bytes() {
    for (bytes, at_end) in [(b"" as &[u8], true), (b"x", false)] {
        let mut slice = bytes;
        assert_eq!(slice.sync_query_end_unpin(), Ok(at_end));
        assert_eq!(slice, bytes);

        let mut shared = Bytes::from_static(bytes);
        assert_eq!(shared.sync_query_end_unpin(), Ok(at_end));
        assert_eq!(shared, bytes);

        let mut mutable = BytesMut::from(bytes);
        assert_eq!(mutable.sync_query_end_unpin(), Ok(at_end));
        assert_eq!(mutable, bytes);

        let mut queued = VecDeque::from(bytes.to_vec());
        assert_eq!(queued.sync_query_end_unpin(), Ok(at_end));
        assert_eq!(queued, bytes);
    }
}

#[cfg(feature = "std")]
#[test]
fn cursor_reports_eof_at_or_past_its_end() {
    let mut cursor = std::io::Cursor::new(*b"ab");
    for (position, at_end) in [(0, false), (1, false), (2, true), (u64::MAX, true)] {
        cursor.set_position(position);
        assert_eq!(cursor.sync_query_end_unpin(), Ok(at_end));
        assert_eq!(cursor.position(), position);
    }
}

#[test]
fn a_read_limit_reaches_eof_at_the_quota_or_the_inner_end() {
    let mut limited = ReadLimit::new(&b"ab"[..], 1u8);
    assert_eq!(limited.sync_query_end_unpin(), Ok(false));
    assert_eq!(limited.sync_read_array_unpin::<1>().unwrap().value, *b"a");
    assert_eq!(limited.sync_query_end_unpin(), Ok(true));
    assert_eq!(limited.into_parts().io, b"b");

    let mut limited = ReadLimit::new(&b"a"[..], 2u8);
    assert_eq!(limited.sync_query_end_unpin(), Ok(false));
    assert_eq!(limited.sync_read_array_unpin::<1>().unwrap().value, *b"a");
    assert_eq!(limited.sync_query_end_unpin(), Ok(true));
    assert_eq!(limited.into_parts().quota, 1);
}

/// The answer becomes available after one Pending.
struct Waiting {
    polled: bool,
    outcome: Result<bool, Reset>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reset;

family_receiver!(impl [] for Waiting);
impl AtEnd for Waiting {
    type RemainderError = Reset;
    fn poll_at_end<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<bool, Reset>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if !this.polled {
            this.polled = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        Poll::Ready(this.outcome)
    }
}

#[derive(Default)]
struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn assert_forwarded(
    query: impl Future<Output = Result<bool, Reset>>,
    outcome: Result<bool, Reset>,
) {
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut query = pin!(query);
    assert_eq!(query.as_mut().poll(&mut cx), Poll::Pending);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert_eq!(query.as_mut().poll(&mut cx), Poll::Ready(outcome));
}

#[call]
async fn query_through_lent_limit<F: AtEnd>(
    source: &mut impl ReceiverAccess<Family = F>,
) -> Result<bool, F::RemainderError> {
    ReadLimit::new(source, 3u8).at_end().await
}

#[test]
fn a_positive_limit_forwards_the_query_on_owned_and_lent_sources() {
    for outcome in [Ok(false), Ok(true), Err(Reset)] {
        let source = || Waiting {
            polled: false,
            outcome,
        };
        assert_forwarded(ReadLimit::new(source(), 3u8).query_end_unpin(), outcome);
        assert_forwarded(source().query_through_lent_limit_unpin(), outcome);
    }
}

#[test]
fn a_zero_quota_answers_without_polling_the_source() {
    let mut limited = ReadLimit::new(
        Waiting {
            polled: false,
            outcome: Err(Reset),
        },
        0u8,
    );
    assert_eq!(limited.sync_query_end_unpin(), Ok(true));
    assert!(!limited.into_parts().io.polled);
}

#[test]
fn wrappers_forward_pending_and_errors_from_an_at_end_only_source() {
    for outcome in [Ok(false), Ok(true), Err(Reset)] {
        let source = || Waiting {
            polled: false,
            outcome,
        };
        assert_forwarded(
            Chunked::new(source(), NonZeroUsize::MIN).query_end_unpin(),
            outcome,
        );
        assert_forwarded(MergedMetadata::new(source(), ()).query_end_unpin(), outcome);
        assert_forwarded(ReadSkip::new(source()).query_end_unpin(), outcome);
        assert_forwarded(ReadChunkSkip::new(source()).query_end_unpin(), outcome);
    }
}

//! The skip capability on memory sources and its forwarding through a loan,
//! a read budget and `Chunked`.
#![feature(impl_trait_in_assoc_type)]
use bytes::{BufMut, Bytes, BytesMut};
use core::{
    num::NonZeroUsize,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use nitori_call::{Call, CallPoll, Lend, Receiver, ReceiverAccess, call};
use nitori_io::{
    Infinite, Min,
    adapters::{Chunked, Limited, ReadLimit},
    calls::SkipExactError,
    prelude::*,
};
use std::{collections::VecDeque, io::Cursor};

#[test]
fn memory_sources_skip_by_advancing() {
    let mut slice = &b"abcdef"[..];
    assert_eq!(slice.sync_skip_unpin(2u8), Ok(Some(2usize)));
    assert_eq!(slice, b"cdef");
    assert_eq!(slice.sync_skip_unpin(Infinite), Ok(Some(4)));
    assert_eq!(slice.sync_skip_unpin(Infinite), Ok(None));

    let mut bytes = Bytes::from_static(b"abcdef");
    assert_eq!(bytes.sync_skip_unpin(4usize), Ok(Some(4)));
    assert_eq!(bytes, &b"ef"[..]);

    let mut bytes = BytesMut::from(&b"abcdef"[..]);
    assert_eq!(bytes.sync_skip_unpin(10u64), Ok(Some(6)));
    assert_eq!(bytes.sync_skip_unpin(10u64), Ok(None));

    let mut deque = VecDeque::from(b"abcdef".to_vec());
    deque.rotate_left(3); // the bytes wrap around the ring
    assert_eq!(deque.sync_skip_unpin(4u16), Ok(Some(4)));
    assert_eq!(deque.iter().copied().collect::<Vec<_>>(), b"bc");

    let mut cursor = Cursor::new(b"abcdef".to_vec());
    assert_eq!(cursor.sync_skip_unpin(3u32), Ok(Some(3)));
    assert_eq!(cursor.position(), 3);
    assert_eq!(cursor.sync_skip_unpin(u128::MAX), Ok(Some(3)));
    assert_eq!(cursor.sync_skip_unpin(1u8), Ok(None));
}

#[test]
fn a_zero_maximum_makes_no_progress_and_eof_stays_eof() {
    let mut slice = &b"ab"[..];
    assert_eq!(slice.sync_skip_unpin(0u8), Ok(Some(0usize)));
    assert_eq!(slice, b"ab");
    let mut empty = &b""[..];
    assert_eq!(empty.sync_skip_unpin(0u8), Ok(None));
}

#[test]
fn chunked_skips_on_its_inner_source_between_chunks() {
    let mut chunked = Chunked::new(&b"abcdef"[..], NonZeroUsize::new(4).unwrap());
    let chunk = chunked
        .sync_read_chunk_unpin(NonZeroUsize::MIN)
        .unwrap()
        .map(|segment| segment.chunk);
    assert_eq!(chunk.as_deref(), Some(&b"a"[..]));
    assert_eq!(chunked.sync_skip_unpin(2u8), Ok(Some(2usize)));
    let chunk = chunked
        .sync_read_chunk_unpin(NonZeroUsize::MAX)
        .unwrap()
        .map(|segment| segment.chunk);
    assert_eq!(chunk.as_deref(), Some(&b"def"[..]));
    assert_eq!(chunked.sync_skip_unpin(Infinite), Ok(None));
}

#[test]
fn a_budget_bounds_skips_and_reports_eof_at_its_boundary() {
    let mut framed = ReadLimit::new(&b"abcdef"[..], 4u8);
    assert_eq!(framed.sync_skip_unpin(Infinite), Ok(Some(4usize)));
    assert_eq!(framed.get_ref(), &&b"ef"[..]);
    assert_eq!(framed.quota(), &0);
    assert_eq!(framed.sync_skip_unpin(Infinite), Ok(None));
    assert_eq!(framed.get_ref(), &&b"ef"[..]);
}

#[test]
fn budgets_and_maxima_of_other_widths_meet_a_usize_count() {
    let mut framed = ReadLimit::new(&b"abcdef"[..], u64::MAX);
    assert_eq!(framed.sync_skip_unpin(2u8), Ok(Some(2usize)));
    assert_eq!(framed.quota(), &(u64::MAX - 2));
    let mut framed = ReadLimit::new(&b"abcdef"[..], Infinite);
    assert_eq!(framed.sync_skip_unpin(5u128), Ok(Some(5usize)));
    assert_eq!(framed.sync_skip_unpin(5u128), Ok(Some(1)));
    assert_eq!(framed.sync_skip_unpin(5u128), Ok(None));
}

#[test]
fn the_boundary_answers_without_polling_the_parent() {
    let mut framed = ReadLimit::new(Failing, 0usize);
    assert_eq!(framed.sync_skip_unpin(Infinite), Ok(None));
    // Within the budget, even an empty request reaches the parent.
    let mut framed = ReadLimit::new(Failing, 1usize);
    assert_eq!(framed.sync_skip_unpin(0usize), Err(Broken));
}

#[test]
fn a_zero_maximum_at_a_zero_budget_answers_none_without_polling_the_parent() {
    let mut framed = ReadLimit::new(Failing, 0usize);
    assert_eq!(framed.sync_skip_unpin(0usize), Ok(None));
    assert_eq!(framed.quota(), &0);
}

#[test]
fn a_host_failure_displays_the_count_left() {
    let error = SkipExactError::Receiver {
        source: Broken,
        remaining: 3usize,
    };
    assert_eq!(error.to_string(), "skip failed with 3 bytes left");
}

/// Skips up to `maximum` within a budget of `budget` composed on the access,
/// through the loan of the access's view, and returns what is left of it.
#[call(sync)]
async fn skip_within<F: Skip, M>(
    io: &mut impl ReceiverAccess<Family = F>,
    budget: u8,
    maximum: M,
) -> (Result<Option<<F as Skip>::Count>, F::Error>, u8)
where
    M: Min<<F as Skip>::Count, Output = <F as Skip>::Count> + Clone,
    u8: Min<<F as Skip>::Count, Output = <F as Skip>::Count>,
    <F as Skip>::Count: Min<u8, Output = u8> + Clone + PartialOrd,
{
    let mut limited = ReadLimit::new(&mut *io, budget);
    let result = limited.skip(maximum).await;
    (result, limited.into_parts().quota)
}

#[test]
fn a_loan_skips_on_the_view_it_lends() {
    let mut input = &b"abcdef"[..];
    assert_eq!(input.sync_skip_within_unpin(3, Infinite), (Ok(Some(3)), 0));
    assert_eq!(input, b"def");
    assert_eq!(input.sync_skip_within_unpin(3, 1u16), (Ok(Some(1)), 2));
    assert_eq!(input, b"ef");
}

/// A source that skips one byte on every second poll, and reads likewise.
struct Delayed<'data> {
    bytes: &'data [u8],
    waiting: bool,
}
nitori_call::family_receiver!(impl ['data] for Delayed<'data>);
impl Delayed<'_> {
    /// Wait on every other poll; then take at most one byte, or none if
    /// `limit` is zero.
    fn take(&mut self, cx: &mut Context<'_>, limit: usize) -> Poll<usize> {
        if !self.waiting {
            self.waiting = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.waiting = false;
        let count = Ord::min(Ord::min(limit, self.bytes.len()), 1);
        self.bytes = &self.bytes[count..];
        Poll::Ready(count)
    }
}
impl Read for Delayed<'_> {
    type Error = !;
    type Metadata = ();
    fn poll_read<'v, B: bytes::BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<Option<nitori_io::ReadSegment<Self::Metadata>>, Self::Error>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let before = this.bytes;
        let count = core::task::ready!(this.take(cx, out.remaining_mut()));
        out.put_slice(&before[..count]);
        Poll::Ready(Ok(segment(count)))
    }
}
impl Skip for Delayed<'_> {
    type Count = usize;
    fn poll_skip<'v, M: Min<usize, Output = usize> + Clone>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<usize>, Self::Error>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let empty = this.bytes.is_empty();
        let count = core::task::ready!(this.take(cx, Min::min(maximum, usize::MAX)));
        Poll::Ready(Ok((!empty).then_some(count)))
    }
}

/// Skips `length` bytes, one poll's skip at a time.
#[call(sync)]
async fn skip_all<F: Skip<Count = usize>>(
    io: &mut impl ReceiverAccess<Family = F>,
    mut length: usize,
) -> Result<(), F::Error> {
    while length > 0 {
        match io.skip(length).await? {
            Some(count) => length -= count,
            None => break,
        }
    }
    Ok(())
}

#[test]
fn a_cancelled_skip_leaves_the_source_and_the_budget_in_step() {
    let mut framed = ReadLimit::new(
        Delayed {
            bytes: b"abcdef",
            waiting: false,
        },
        5usize,
    );
    let mut call = Box::pin(Call::<Limited<Delayed<'_>, usize>, _>::new(SkipAll::new(4)));
    let mut cx = Context::from_waker(Waker::noop());
    // The second poll skips the first byte, then waits for the second.
    for _ in 0..2 {
        let poll = call.as_mut().poll_with(
            Some(Lend::Fixed(pin!(Pin::new(&mut framed).view()))),
            &mut cx,
        );
        assert!(matches!(poll, CallPoll::Pending));
    }
    drop(call);
    let source = framed.get_ref();
    let budget = framed.quota();
    assert_eq!(source.bytes, b"bcdef");
    assert_eq!(budget, &4);
    let mut call = pin!(Call::<Limited<Delayed<'_>, usize>, _>::new(SkipAll::new(4)));
    let result = loop {
        let poll = call.as_mut().poll_with(
            Some(Lend::Fixed(pin!(Pin::new(&mut framed).view()))),
            &mut cx,
        );
        match poll {
            CallPoll::Pending => continue,
            CallPoll::Ready(ControlFlow::Break(result)) => break result,
            _ => panic!("the call neither waits nor finishes"),
        }
    };
    assert_eq!(result, Ok(()));
    let source = framed.get_ref();
    let budget = framed.quota();
    assert_eq!((source.bytes, budget), (&b"f"[..], &0));
}

#[test]
fn a_cancelled_skip_through_chunked_leaves_the_source_and_the_budget_in_step() {
    type Framed<'data> = Limited<nitori_call::Direct<Chunked<Delayed<'data>>>, usize>;
    let mut framed = ReadLimit::new(
        Chunked::new(
            Delayed {
                bytes: b"abcdef",
                waiting: false,
            },
            NonZeroUsize::MIN,
        ),
        5usize,
    );
    let mut call = Box::pin(Call::<Framed<'_>, _>::new(SkipAll::new(4)));
    let mut cx = Context::from_waker(Waker::noop());
    // The second poll skips the first byte, then waits for the second.
    for _ in 0..2 {
        let poll = call.as_mut().poll_with(
            Some(Lend::Fixed(pin!(Pin::new(&mut framed).view()))),
            &mut cx,
        );
        assert!(matches!(poll, CallPoll::Pending));
    }
    drop(call);
    let chunked = framed.get_ref();
    let budget = framed.quota();
    assert_eq!(chunked.get_ref().bytes, b"bcdef");
    assert_eq!(budget, &4);
    let mut call = pin!(Call::<Framed<'_>, _>::new(SkipAll::new(4)));
    let result = loop {
        let poll = call.as_mut().poll_with(
            Some(Lend::Fixed(pin!(Pin::new(&mut framed).view()))),
            &mut cx,
        );
        match poll {
            CallPoll::Pending => continue,
            CallPoll::Ready(ControlFlow::Break(result)) => break result,
            _ => panic!("the call neither waits nor finishes"),
        }
    };
    assert_eq!(result, Ok(()));
    let chunked = framed.get_ref();
    let budget = framed.quota();
    assert_eq!((chunked.get_ref().bytes, budget), (&b"f"[..], &0));
}

/// Every capability fails.
struct Failing;
#[derive(Debug, PartialEq)]
struct Broken;
nitori_call::family_receiver!(impl [] for Failing);
impl Read for Failing {
    type Error = Broken;
    type Metadata = ();
    fn poll_read<'v, B: bytes::BufMut + ?Sized>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        _: &mut B,
    ) -> Poll<Result<Option<nitori_io::ReadSegment<Self::Metadata>>, Broken>>
    where
        Self: 'v,
    {
        Poll::Ready(Err(Broken))
    }
}
impl Skip for Failing {
    type Count = usize;
    fn poll_skip<'v, M: Min<usize, Output = usize> + Clone>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        _: M,
    ) -> Poll<Result<Option<usize>, Broken>>
    where
        Self: 'v,
    {
        Poll::Ready(Err(Broken))
    }
}

/// Reports one byte more than it was allowed to skip.
struct Overskip;
nitori_call::family_receiver!(impl [] for Overskip);
impl Read for Overskip {
    type Error = !;
    type Metadata = ();
    fn poll_read<'v, B: BufMut + ?Sized>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        _: &mut B,
    ) -> Poll<Result<Option<nitori_io::ReadSegment<Self::Metadata>>, !>>
    where
        Self: 'v,
    {
        Poll::Ready(Ok(None))
    }
}
impl Skip for Overskip {
    type Count = usize;
    fn poll_skip<'v, M: Min<usize, Output = usize> + Clone>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<usize>, !>>
    where
        Self: 'v,
    {
        Poll::Ready(Ok(Some(Min::min(maximum, usize::MAX) + 1)))
    }
}

#[test]
#[should_panic = "skip exceeded the requested maximum"]
fn a_parent_skipping_past_the_budget_panics() {
    let mut framed = ReadLimit::new(Overskip, 2usize);
    let _ = framed.sync_skip_unpin(Infinite);
}

fn segment(count: usize) -> Option<nitori_io::ReadSegment<()>> {
    core::num::NonZeroUsize::new(count).map(|count| nitori_io::ReadSegment {
        count,
        metadata: (),
    })
}

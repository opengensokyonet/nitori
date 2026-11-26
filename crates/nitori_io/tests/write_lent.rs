#![feature(impl_trait_in_assoc_type)]
//! Lending a sink's storage: what each sink keeps, what a fill that gives up
//! leaves behind, the lent buffer's own contract, and the forwarding
//! adapters.
use bytes::{Buf as _, BufMut, BytesMut};
use nitori_call::{Receiver, ReceiverAccess, call, family_receiver};
use nitori_io::Filled;
use nitori_io::{
    LentBuffer, LentOutcome, LentSegment, Write, WriteLent, WriteSegment,
    adapters::{Chunked, WriteLimit},
    error::InsufficientCapacityError,
    prelude::*,
};
use std::{
    cell::Cell,
    fmt::Debug,
    future::poll_fn,
    num::NonZeroUsize,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}
fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}
fn ready<T, E: Debug>(poll: Poll<Result<T, E>>) -> T {
    match poll {
        Poll::Ready(result) => result.unwrap(),
        Poll::Pending => panic!("unexpected Pending"),
    }
}

/// What one lend came to: the sink's grant is the fill's value.
#[derive(Debug, PartialEq, Eq)]
enum Lend {
    /// The sink accepts no byte; the fill was not called.
    None,
    /// The fill put `count` bytes into a grant of `granted` and the sink kept them.
    Kept { granted: usize, count: usize },
    /// The fill returned, but the sink kept nothing.
    NotKept { granted: usize },
    /// The fill gave up.
    GaveUp,
}
fn lend_of(outcome: Option<LentOutcome<(), usize, &'static str>>) -> Lend {
    match outcome {
        None => Lend::None,
        Some(LentOutcome::Kept(segment)) => Lend::Kept {
            granted: segment.value,
            count: segment.count.get(),
        },
        Some(LentOutcome::NotKept(granted)) => Lend::NotKept { granted },
        Some(LentOutcome::GaveUp(reason)) => {
            assert_eq!(reason, "gave up");
            Lend::GaveUp
        }
    }
}

/// Lend `sink` for at most `wanted` bytes and put a prefix of `bytes` in the
/// grant. `called` counts the calls of the fill.
fn lend_put<S>(sink: &mut S, wanted: usize, bytes: &[u8], called: &Cell<usize>) -> Lend
where
    S: Receiver + Unpin,
    S::Family: WriteLent<(), Report = ()>,
    <S::Family as Write<()>>::Error: Debug,
{
    let polled = Pin::new(sink).poll_write_lent(&mut cx(), nz(wanted), |lent| {
        called.set(called.get() + 1);
        let granted = lent.granted().get();
        lent.buffer().put_slice(&bytes[..bytes.len().min(granted)]);
        Ok::<_, &'static str>(Filled {
            value: granted,
            options: (),
        })
    });
    lend_of(ready(polled))
}

/// Like [lend_put], but the fill puts `bytes` and then gives up.
fn lend_give_up<S>(sink: &mut S, wanted: usize, bytes: &[u8]) -> Lend
where
    S: Receiver + Unpin,
    S::Family: WriteLent<(), Report = ()>,
    <S::Family as Write<()>>::Error: Debug,
{
    let polled = Pin::new(sink).poll_write_lent(&mut cx(), nz(wanted), |lent| {
        let granted = lent.granted().get();
        lent.buffer().put_slice(&bytes[..bytes.len().min(granted)]);
        Err::<Filled<usize, ()>, _>("gave up")
    });
    lend_of(ready(polled))
}

#[test]
fn bytes_mut_keeps_the_three_bytes_the_fill_put() {
    let mut sink = BytesMut::new();
    let called = Cell::new(0);
    let lend = lend_put(&mut sink, 8, b"abc", &called);
    let Lend::Kept { granted, count } = lend else {
        panic!("expected a kept segment, got {lend:?}");
    };
    assert_eq!(count, 3);
    assert!((3..=8).contains(&granted));
    assert_eq!(called.get(), 1);
    assert_eq!(&sink[..], b"abc");
}

#[test]
fn vec_keeps_the_three_bytes_after_its_existing_prefix() {
    let mut sink = b"xy".to_vec();
    let called = Cell::new(0);
    let lend = lend_put(&mut sink, 8, b"abc", &called);
    let Lend::Kept { granted, count } = lend else {
        panic!("expected a kept segment, got {lend:?}");
    };
    assert_eq!(count, 3);
    assert!((3..=8).contains(&granted));
    assert_eq!(sink, b"xyabc");
}

#[test]
fn the_grant_never_exceeds_what_was_wanted() {
    let mut vector = Vec::new();
    let mut bytes = BytesMut::new();
    let called = Cell::new(0);
    for (wanted, put) in [(1, b"abcdef"), (2, b"abcdef"), (5, b"abcdef")] {
        let Lend::Kept { granted, count } = lend_put(&mut vector, wanted, put, &called) else {
            panic!("a growable sink lends");
        };
        assert!(granted <= wanted);
        assert_eq!(count, granted.min(put.len()));
        let Lend::Kept { granted, count } = lend_put(&mut bytes, wanted, put, &called) else {
            panic!("a growable sink lends");
        };
        assert!(granted <= wanted);
        assert_eq!(count, granted.min(put.len()));
    }
}

#[test]
fn a_fill_that_gives_up_after_putting_bytes_leaves_bytes_mut_unchanged() {
    let mut sink = BytesMut::from(&b"xy"[..]);
    assert_eq!(lend_give_up(&mut sink, 8, b"abc"), Lend::GaveUp);
    assert_eq!(&sink[..], b"xy");
    // The sink is still usable, and keeps the next fill.
    let called = Cell::new(0);
    assert!(matches!(
        lend_put(&mut sink, 8, b"z", &called),
        Lend::Kept { count: 1, .. }
    ));
    assert_eq!(&sink[..], b"xyz");
}

#[test]
fn a_fill_that_gives_up_after_putting_bytes_leaves_vec_unchanged() {
    let mut sink = b"xy".to_vec();
    assert_eq!(lend_give_up(&mut sink, 8, b"abc"), Lend::GaveUp);
    assert_eq!(sink, b"xy");
    let mut empty = Vec::new();
    assert_eq!(lend_give_up(&mut empty, 8, b"abc"), Lend::GaveUp);
    assert!(empty.is_empty());
}

#[test]
fn a_slice_advances_past_the_bytes_it_keeps() {
    let mut storage = [0; 5];
    let mut sink = &mut storage[..];
    let called = Cell::new(0);
    let Lend::Kept { granted, count } = lend_put(&mut sink, 8, b"ab", &called) else {
        panic!("a slice with room lends");
    };
    // Lends the rest of the slice, at most what was wanted.
    assert_eq!((granted, count), (5, 2));
    assert_eq!(sink.len(), 3);
    assert_eq!(
        lend_put(&mut sink, 2, b"cdefg", &called),
        Lend::Kept {
            granted: 2,
            count: 2
        }
    );
    assert_eq!(sink.len(), 1);
    assert_eq!(called.get(), 2);
    assert_eq!(&storage[..4], b"abcd");
}

#[test]
fn a_used_up_slice_answers_none_without_calling_the_fill() {
    let mut storage = [0; 2];
    let mut sink = &mut storage[..];
    let called = Cell::new(0);
    assert!(matches!(
        lend_put(&mut sink, 8, b"ab", &called),
        Lend::Kept { count: 2, .. }
    ));
    assert!(sink.is_empty());
    assert_eq!(called.get(), 1);
    assert_eq!(lend_put(&mut sink, 8, b"cd", &called), Lend::None);
    assert_eq!(called.get(), 1);
    assert_eq!(storage, *b"ab");
}

#[test]
fn a_slice_does_not_advance_when_the_fill_gives_up() {
    let mut storage = [0; 4];
    let mut sink = &mut storage[..];
    assert_eq!(lend_give_up(&mut sink, 8, b"ab"), Lend::GaveUp);
    assert_eq!(sink.len(), 4);
    // The next fill starts at the same place.
    let called = Cell::new(0);
    assert!(matches!(
        lend_put(&mut sink, 8, b"xy", &called),
        Lend::Kept { count: 2, .. }
    ));
    assert_eq!(&storage[..2], b"xy");
}

// The lent buffer.

#[test]
fn a_lent_buffer_over_too_small_storage_reports_required_and_available() {
    let mut storage = [0; 2];
    let error: InsufficientCapacityError = LentBuffer::new(&mut storage[..], nz(3)).unwrap_err();
    assert_eq!((error.required(), error.available()), (3, 2));
    // Exactly enough is fine.
    let lent = LentBuffer::new(&mut storage[..], nz(2)).unwrap();
    assert_eq!(lent.granted(), nz(2));
}

#[test]
fn a_lent_buffer_counts_what_was_put() {
    let mut storage = Vec::new();
    let mut lent = LentBuffer::new(&mut storage, nz(4)).unwrap();
    assert_eq!((lent.granted(), lent.filled()), (nz(4), 0));
    lent.buffer().put_slice(b"ab");
    assert_eq!(lent.filled(), 2);
    lent.buffer().put_slice(b"cd");
    assert_eq!(lent.filled(), 4);
    // The granted count is the limit of the buffer.
    assert_eq!(lent.buffer().remaining_mut(), 0);
    assert_eq!(*lent.into_storage(), b"abcd");
}

#[test]
fn a_lent_buffer_nothing_was_put_into_counts_zero_and_hands_back_its_storage() {
    let mut storage = b"xy".to_vec();
    let lent = LentBuffer::new(&mut storage, nz(4)).unwrap();
    assert_eq!((lent.granted(), lent.filled()), (nz(4), 0));
    assert_eq!(*lent.into_storage(), b"xy");
}

#[test]
fn a_lent_buffer_whose_storage_shrank_counts_zero_filled() {
    let mut storage = b"abc".to_vec();
    let mut lent = LentBuffer::new(&mut storage, nz(2)).unwrap();
    lent.buffer().get_mut().clear();
    assert_eq!(lent.filled(), 0);
}

#[test]
fn a_lent_buffer_debug_shows_the_granted_and_filled_counts() {
    let mut storage = Vec::new();
    let mut lent = LentBuffer::new(&mut storage, nz(4)).unwrap();
    lent.buffer().put_slice(b"ab");
    let shown = format!("{lent:?}");
    assert!(shown.contains("LentBuffer"), "{shown}");
    assert!(shown.contains("granted: 4"), "{shown}");
    assert!(shown.contains("filled: 2"), "{shown}");
}

#[test]
fn a_fill_that_puts_nothing_is_not_kept_and_leaves_the_sink_unchanged() {
    let called = Cell::new(0);
    let mut vector = b"xy".to_vec();
    assert_eq!(
        lend_put(&mut vector, 8, b"", &called),
        Lend::NotKept { granted: 8 }
    );
    assert_eq!(vector, b"xy");
    let mut bytes = BytesMut::from(&b"xy"[..]);
    assert!(matches!(
        lend_put(&mut bytes, 8, b"", &called),
        Lend::NotKept { .. }
    ));
    assert_eq!(&bytes[..], b"xy");
    let mut storage = [0; 4];
    let mut slice = &mut storage[..];
    assert_eq!(
        lend_put(&mut slice, 3, b"", &called),
        Lend::NotKept { granted: 3 }
    );
    // The slice does not advance, so the next fill starts at the same place.
    assert_eq!(slice.len(), 4);
    assert!(matches!(
        lend_put(&mut slice, 8, b"ab", &called),
        Lend::Kept { count: 2, .. }
    ));
    assert_eq!(called.get(), 4);
    assert_eq!(&storage[..2], b"ab");
}

#[test]
fn a_fill_that_raises_the_limit_and_overfills_is_not_kept() {
    // A vector lends a slice of exactly the grant, so it cannot be overfilled.
    let mut bytes = BytesMut::from(&b"xy"[..]);
    let outcome = ready(
        Pin::new(&mut bytes).poll_write_lent(&mut cx(), nz(2), |lent| {
            lent.buffer().set_limit(10);
            lent.buffer().put_slice(b"abcde");
            Ok::<_, !>(Filled {
                value: "value",
                options: (),
            })
        }),
    );
    assert!(matches!(outcome, Some(LentOutcome::NotKept("value"))));
    assert_eq!(&bytes[..], b"xy");
}

// Composition and forwarding.

/// A sink that records how it was asked to lend.
#[derive(Default)]
struct Spy {
    bytes: Vec<u8>,
    polls: Rc<Cell<usize>>,
    last_wanted: Rc<Cell<usize>>,
}
family_receiver!(impl [] for Spy);
impl Write<()> for Spy {
    type Error = !;
    type Report = ();
    fn poll_write<'v, B: bytes::Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut B,
        _: &(),
    ) -> Poll<Result<Option<WriteSegment<()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        this.polls.set(this.polls.get() + 1);
        let count = input.remaining();
        let mut copy = vec![0; count];
        input.copy_to_slice(&mut copy);
        this.bytes.extend(copy);
        Poll::Ready(Ok(
            NonZeroUsize::new(count).map(|count| WriteSegment { count, report: () })
        ))
    }
}
impl WriteLent<()> for Spy {
    type Buffer<'l> = &'l mut Vec<u8>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<Result<Option<LentOutcome<(), T, A>>, !>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<&'l mut Vec<u8>>) -> Result<Filled<T, ()>, A>,
    {
        let this = host.get_pin_mut().get_mut();
        this.polls.set(this.polls.get() + 1);
        this.last_wanted.set(wanted.get());
        let before = this.bytes.len();
        let mut lent = LentBuffer::new(&mut this.bytes, wanted).expect("a vector has room");
        let result = fill(&mut lent);
        let count = lent
            .into_storage()
            .len()
            .checked_sub(before)
            .filter(|count| *count <= wanted.get())
            .and_then(NonZeroUsize::new);
        let outcome = match (result, count) {
            (Ok(Filled { value, .. }), Some(count)) => LentOutcome::Kept(LentSegment {
                count,
                report: (),
                value,
            }),
            (Ok(Filled { value, .. }), None) => LentOutcome::NotKept(value),
            (Err(abandoned), _) => LentOutcome::GaveUp(abandoned),
        };
        if !matches!(outcome, LentOutcome::Kept(_)) {
            this.bytes.truncate(before);
        }
        Poll::Ready(Ok(Some(outcome)))
    }
}

/// Lend through an access, as an operation does.
async fn lend_through<A>(io: &mut A, wanted: usize, bytes: &[u8], called: &Cell<usize>) -> Lend
where
    A: ReceiverAccess<Family: WriteLent<(), Report = (), Error = !>>,
{
    let outcome = poll_fn(|cx| {
        io.poll_access(cx, |view, cx| {
            let polled = <A::Family as WriteLent<()>>::poll_write_lent(
                view.into_pin(),
                cx,
                nz(wanted),
                |lent| {
                    called.set(called.get() + 1);
                    let granted = lent.granted().get();
                    lent.buffer().put_slice(&bytes[..bytes.len().min(granted)]);
                    Ok::<_, &'static str>(Filled {
                        value: granted,
                        options: (),
                    })
                },
            );
            polled.map(|result| result.unwrap())
        })
    })
    .await;
    lend_of(outcome)
}

#[call(sync)]
async fn lend_limited<F: WriteLent<(), Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
    budget: usize,
    wanted: usize,
    rounds: usize,
    called: &Cell<usize>,
) -> (Vec<Lend>, usize) {
    let mut limited = io.compose(WriteLimit(budget));
    let mut lends = Vec::new();
    for _ in 0..rounds {
        lends.push(lend_through(&mut limited, wanted, b"abcdefgh", called).await);
    }
    (lends, limited.into_state().0)
}

#[test]
fn a_write_limit_grants_no_more_than_the_budget_that_remains() {
    let mut sink = BytesMut::new();
    let called = Cell::new(0);
    let (lends, left) = sink.sync_lend_limited_unpin(5, 4, 3, &called);
    assert_eq!(
        lends,
        [
            Lend::Kept {
                granted: 4,
                count: 4
            },
            Lend::Kept {
                granted: 1,
                count: 1
            },
            Lend::None,
        ]
    );
    assert_eq!(left, 0);
    assert_eq!(called.get(), 2);
    assert_eq!(&sink[..], b"abcda");
}

#[test]
fn a_write_limit_passes_the_budget_down_as_what_is_wanted() {
    let mut sink = Spy::default();
    let last_wanted = sink.last_wanted.clone();
    let called = Cell::new(0);
    let (lends, left) = sink.sync_lend_limited_unpin(3, 8, 1, &called);
    assert_eq!(
        lends,
        [Lend::Kept {
            granted: 3,
            count: 3
        }]
    );
    assert_eq!((left, last_wanted.get()), (0, 3));
}

#[test]
fn an_exhausted_write_limit_answers_none_without_the_fill_or_the_parent() {
    let mut sink = Spy::default();
    let polls = sink.polls.clone();
    let called = Cell::new(0);
    let (lends, left) = sink.sync_lend_limited_unpin(0, 8, 2, &called);
    assert_eq!(lends, [Lend::None, Lend::None]);
    assert_eq!((left, called.get(), polls.get()), (0, 0, 0));
    assert!(sink.bytes.is_empty());
}

#[test]
fn a_write_limit_stops_polling_the_parent_once_the_budget_is_spent() {
    let mut sink = Spy::default();
    let polls = sink.polls.clone();
    let called = Cell::new(0);
    let (lends, _) = sink.sync_lend_limited_unpin(2, 8, 3, &called);
    assert_eq!(lends.last(), Some(&Lend::None));
    assert_eq!((polls.get(), called.get()), (1, 1));
}

#[test]
fn chunked_forwards_lending_to_the_sink_it_wraps() {
    let mut sink = Chunked::new(BytesMut::from(&b"xy"[..]), nz(4));
    let called = Cell::new(0);
    let Lend::Kept { granted, count } = lend_put(&mut sink, 8, b"abc", &called) else {
        panic!("the wrapped sink lends");
    };
    assert_eq!(count, 3);
    // Not capped by the chunk capacity, which bounds reads.
    assert!((3..=8).contains(&granted));
    assert_eq!(&sink.get_ref()[..], b"xyabc");
    assert_eq!(lend_give_up(&mut sink, 8, b"zz"), Lend::GaveUp);
    assert_eq!(&sink.into_inner()[..], b"xyabc");
}

#[test]
fn chunked_over_a_used_up_slice_answers_none() {
    let mut storage = [0; 0];
    let mut sink = Chunked::new(&mut storage[..], nz(4));
    let called = Cell::new(0);
    assert_eq!(lend_put(&mut sink, 8, b"a", &called), Lend::None);
    assert_eq!(called.get(), 0);
}

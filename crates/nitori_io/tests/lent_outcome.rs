#![feature(impl_trait_in_assoc_type)]
//! What the sinks that lend make of a loan the fill broke: a buffer swapped
//! for one of the fill's own, bytes put past the granted count, storage
//! shortened, or nothing put at all. The sink counts what it keeps from its
//! own storage, answers NotKept and leaves its content as it was, and does
//! not panic.
use bytes::{BufMut, BytesMut};
use nitori_call::{Receiver, ReceiverAccess, call};
use nitori_io::{
    Filled, LentBuffer, LentOutcome,
    adapters::{ReadChunkSkip, ReadSkip, WriteLimit},
    prelude::*,
};
use std::{
    cell::Cell,
    future::poll_fn,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, Waker},
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}
fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

type Outcome = Option<LentOutcome<(), &'static str, &'static str>>;

/// What an outcome came to, with the fill's value or the count kept.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    /// The sink accepts no byte.
    Nothing,
    Kept(usize, &'static str),
    NotKept(&'static str),
    GaveUp(&'static str),
}
fn kind(outcome: Outcome) -> Kind {
    match outcome {
        None => Kind::Nothing,
        Some(LentOutcome::Kept(segment)) => Kind::Kept(segment.count.get(), segment.value),
        Some(LentOutcome::NotKept(value)) => Kind::NotKept(value),
        Some(LentOutcome::GaveUp(reason)) => Kind::GaveUp(reason),
    }
}

/// Lend the sink for at most "wanted" bytes to the fill.
fn lend<S>(
    sink: &mut S,
    wanted: usize,
    fill: impl for<'l> FnOnce(
        &mut LentBuffer<<S::Family as WriteLent<()>>::Buffer<'l>>,
    ) -> Result<&'static str, &'static str>,
) -> Outcome
where
    S: Receiver + Unpin,
    S::Family: WriteLent<(), Report = (), Error = !>,
{
    let polled = Pin::new(sink).poll_write_lent(&mut cx(), nz(wanted), |lent| {
        fill(lent).map(|value| Filled { value, options: () })
    });
    match polled {
        Poll::Ready(Ok(outcome)) => outcome,
        Poll::Ready(Err(never)) => never,
        Poll::Pending => panic!("unexpected Pending"),
    }
}

/// A well-behaved fill that puts "bytes" in the grant.
fn put<S>(sink: &mut S, bytes: &'static [u8]) -> Kind
where
    S: Receiver + Unpin,
    S::Family: WriteLent<(), Report = (), Error = !>,
{
    kind(lend(sink, 8, |lent| {
        lent.buffer().put_slice(bytes);
        Ok("put")
    }))
}

// A BytesMut lends a buffer split off after the written bytes: it starts
// empty, and what is kept is what it holds when the fill returns.
mod bytes_mut {
    use super::*;

    fn make(prefix: &[u8]) -> BytesMut {
        BytesMut::from(prefix)
    }

    #[test]
    fn a_fill_that_swaps_in_a_buffer_of_its_own_is_not_kept() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 4, |lent| {
            let foreign: &'static mut BytesMut = Box::leak(Box::new(BytesMut::new()));
            *lent = LentBuffer::new(foreign, lent.granted()).unwrap();
            lent.buffer().put_slice(b"abc");
            Ok("swapped")
        });
        assert_eq!(kind(outcome), Kind::NotKept("swapped"));
        assert_eq!(&sink[..], b"xy");
        // The sink is still usable.
        assert_eq!(put(&mut sink, b"z"), Kind::Kept(1, "put"));
        assert_eq!(&sink[..], b"xyz");
    }

    #[test]
    fn bytes_put_past_the_granted_count_through_get_mut_are_not_kept() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 2, |lent| {
            lent.buffer().get_mut().extend_from_slice(b"abcde");
            Ok("over")
        });
        assert_eq!(kind(outcome), Kind::NotKept("over"));
        assert_eq!(&sink[..], b"xy");
        assert_eq!(put(&mut sink, b"z"), Kind::Kept(1, "put"));
    }

    #[test]
    fn one_byte_past_the_granted_count_is_not_kept() {
        let mut sink = make(b"");
        let outcome = lend(&mut sink, 3, |lent| {
            lent.buffer().get_mut().extend_from_slice(b"abcd");
            Ok("over")
        });
        assert_eq!(kind(outcome), Kind::NotKept("over"));
        assert!(sink.is_empty());
    }

    #[test]
    fn exactly_the_granted_count_put_through_get_mut_is_kept() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 3, |lent| {
            lent.buffer().get_mut().extend_from_slice(b"abc");
            Ok("exact")
        });
        assert_eq!(kind(outcome), Kind::Kept(3, "exact"));
        assert_eq!(&sink[..], b"xyabc");
    }

    #[test]
    fn storage_shortened_through_get_mut_keeps_what_is_left_of_it() {
        let mut sink = make(b"xyz");
        let outcome = lend(&mut sink, 4, |lent| {
            lent.buffer().get_mut().extend_from_slice(b"abc");
            lent.buffer().get_mut().truncate(1);
            Ok("shortened")
        });
        assert_eq!(kind(outcome), Kind::Kept(1, "shortened"));
        assert_eq!(&sink[..], b"xyza");
    }

    #[test]
    fn storage_shortened_when_nothing_was_put_is_not_kept() {
        let mut sink = make(b"xyz");
        let outcome = lend(&mut sink, 4, |lent| {
            // The lent buffer is empty, so this cannot remove written bytes.
            lent.buffer().get_mut().truncate(1);
            Ok("shortened")
        });
        assert_eq!(kind(outcome), Kind::NotKept("shortened"));
        assert_eq!(&sink[..], b"xyz");
        assert_eq!(put(&mut sink, b"q"), Kind::Kept(1, "put"));
        assert_eq!(&sink[..], b"xyzq");
    }

    #[test]
    fn a_fill_that_gives_up_after_overfilling_leaves_the_sink_unchanged() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 2, |lent| {
            lent.buffer().get_mut().extend_from_slice(b"abcde");
            Err("gave up")
        });
        assert_eq!(kind(outcome), Kind::GaveUp("gave up"));
        assert_eq!(&sink[..], b"xy");
    }

    #[test]
    fn a_fill_that_gives_up_after_putting_bytes_leaves_the_sink_unchanged() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 4, |lent| {
            lent.buffer().put_slice(b"ab");
            Err("gave up")
        });
        assert_eq!(kind(outcome), Kind::GaveUp("gave up"));
        assert_eq!(&sink[..], b"xy");
    }

    #[test]
    fn a_fill_that_puts_nothing_is_not_kept_and_is_called_once() {
        let mut sink = make(b"xy");
        let called = Cell::new(0);
        let outcome = lend(&mut sink, 4, |_| {
            called.set(called.get() + 1);
            Ok("nothing put")
        });
        assert_eq!(kind(outcome), Kind::NotKept("nothing put"));
        assert_eq!(called.get(), 1);
        assert_eq!(&sink[..], b"xy");
    }

    #[test]
    fn the_lent_buffer_starts_empty_and_does_not_show_the_written_bytes() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 4, |lent| {
            assert!(lent.buffer().get_mut().is_empty());
            lent.buffer().put_slice(b"ab");
            assert_eq!(&lent.buffer().get_mut()[..], b"ab");
            Ok("put")
        });
        assert_eq!(kind(outcome), Kind::Kept(2, "put"));
    }

    #[test]
    fn kept_bytes_follow_the_bytes_written_before() {
        let mut sink = make(b"xy");
        assert_eq!(put(&mut sink, b"ab"), Kind::Kept(2, "put"));
        assert_eq!(put(&mut sink, b"c"), Kind::Kept(1, "put"));
        assert_eq!(&sink[..], b"xyabc");
    }

    #[test]
    fn a_fill_that_clears_the_lent_buffer_is_not_kept() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 4, |lent| {
            lent.buffer().put_slice(b"ab");
            lent.buffer().get_mut().clear();
            Ok("cleared")
        });
        assert_eq!(kind(outcome), Kind::NotKept("cleared"));
        assert_eq!(&sink[..], b"xy");
    }
}

// A Vec lends a zeroed region after the written bytes as a slice, and is
// judged like a slice: the end must stay, and at most the grant may be used.
mod vec {
    use super::*;

    fn make(prefix: &[u8]) -> Vec<u8> {
        prefix.to_vec()
    }

    #[test]
    fn exactly_the_granted_count_put_through_get_mut_is_kept() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 3, |lent| {
            BufMut::put_slice(lent.buffer().get_mut(), b"abc");
            Ok("exact")
        });
        assert_eq!(kind(outcome), Kind::Kept(3, "exact"));
        assert_eq!(&sink[..], b"xyabc");
    }

    #[test]
    fn the_lent_capacity_holds_only_the_grant() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 3, |lent| {
            assert_eq!(lent.buffer().get_mut().remaining_mut(), 3);
            lent.buffer().put_slice(b"a");
            assert_eq!(lent.filled(), 1);
            Ok("put")
        });
        assert_eq!(kind(outcome), Kind::Kept(1, "put"));
        assert_eq!(&sink[..], b"xya");
    }

    #[test]
    fn a_fill_that_gives_up_after_putting_bytes_leaves_the_sink_unchanged() {
        let mut sink = make(b"xy");
        let outcome = lend(&mut sink, 4, |lent| {
            lent.buffer().put_slice(b"abcd");
            Err("gave up")
        });
        assert_eq!(kind(outcome), Kind::GaveUp("gave up"));
        assert_eq!(&sink[..], b"xy");
        // What was put stays outside the length: the next fill writes over it.
        assert_eq!(put(&mut sink, b"z"), Kind::Kept(1, "put"));
        assert_eq!(&sink[..], b"xyz");
    }

    #[test]
    fn a_fill_that_puts_nothing_is_not_kept_and_is_called_once() {
        let mut sink = make(b"xy");
        let called = Cell::new(0);
        let outcome = lend(&mut sink, 4, |_| {
            called.set(called.get() + 1);
            Ok("nothing put")
        });
        assert_eq!(kind(outcome), Kind::NotKept("nothing put"));
        assert_eq!(called.get(), 1);
        assert_eq!(&sink[..], b"xy");
    }

    #[test]
    fn kept_loans_append_after_the_written_bytes() {
        let mut sink = make(b"xy");
        assert_eq!(put(&mut sink, b"ab"), Kind::Kept(2, "put"));
        assert_eq!(sink.len(), 4);
        assert_eq!(put(&mut sink, b"c"), Kind::Kept(1, "put"));
        assert_eq!(&sink[..], b"xyabc");
    }

    #[test]
    fn an_empty_vector_lends_and_keeps() {
        let mut sink = Vec::new();
        assert_eq!(put(&mut sink, b"abc"), Kind::Kept(3, "put"));
        assert_eq!(&sink[..], b"abc");
    }
}

// A slice keeps what it can count from the slice it has left.

#[test]
fn a_slice_is_not_advanced_by_a_fill_that_swaps_in_a_buffer_of_its_own() {
    let mut storage = [0; 6];
    let mut sink = &mut storage[..];
    let outcome = lend(&mut sink, 3, |lent| {
        let foreign: &'static mut [u8] = Box::leak(vec![0; 8].into_boxed_slice());
        *lent = LentBuffer::new(foreign, lent.granted()).unwrap();
        lent.buffer().put_slice(b"abc");
        Ok("swapped")
    });
    assert_eq!(kind(outcome), Kind::NotKept("swapped"));
    assert_eq!(sink.len(), 6);
    assert_eq!(put(&mut sink, b"z"), Kind::Kept(1, "put"));
    assert_eq!(sink.len(), 5);
    assert_eq!(storage, *b"z\0\0\0\0\0");
}

#[test]
fn a_slice_is_not_advanced_past_the_granted_count_through_get_mut() {
    let mut storage = [0; 6];
    let mut sink = &mut storage[..];
    let outcome = lend(&mut sink, 2, |lent| {
        BufMut::put_slice(lent.buffer().get_mut(), b"abcd");
        Ok("over")
    });
    assert_eq!(kind(outcome), Kind::NotKept("over"));
    assert_eq!(sink.len(), 6);
    // The bytes beyond the grant are written but not kept: the next fill
    // starts at the same place and overwrites them.
    assert_eq!(put(&mut sink, b"zz"), Kind::Kept(2, "put"));
    assert_eq!(&storage[..2], b"zz");
}

#[test]
fn a_slice_is_not_advanced_when_the_fill_swaps_in_a_shorter_prefix_of_it() {
    let mut storage = [0; 6];
    let mut sink = &mut storage[..];
    let outcome = lend(&mut sink, 3, |lent| {
        let rest = std::mem::take(lent.buffer().get_mut());
        *lent.buffer().get_mut() = &mut rest[..2];
        Ok("prefix")
    });
    assert_eq!(kind(outcome), Kind::NotKept("prefix"));
    assert_eq!(sink.len(), 6);
    assert_eq!(put(&mut sink, b"ab"), Kind::Kept(2, "put"));
    assert_eq!(&storage[..2], b"ab");
}

#[test]
fn a_slice_is_not_advanced_when_the_fill_swaps_in_an_empty_slice() {
    let mut storage = [0; 4];
    let mut sink = &mut storage[..];
    let outcome = lend(&mut sink, 3, |lent| {
        *lent.buffer().get_mut() = &mut [];
        Ok("emptied")
    });
    assert_eq!(kind(outcome), Kind::NotKept("emptied"));
    assert_eq!(sink.len(), 4);
}

#[test]
fn a_slice_keeps_exactly_the_granted_count_put_through_get_mut() {
    let mut storage = [0; 6];
    let mut sink = &mut storage[..];
    let outcome = lend(&mut sink, 2, |lent| {
        BufMut::put_slice(lent.buffer().get_mut(), b"ab");
        Ok("exact")
    });
    assert_eq!(kind(outcome), Kind::Kept(2, "exact"));
    assert_eq!(sink.len(), 4);
    assert_eq!(&storage[..2], b"ab");
}

#[test]
fn a_slice_fill_that_puts_nothing_is_not_kept_and_the_slice_stays() {
    let mut storage = [0; 3];
    let mut sink = &mut storage[..];
    let outcome = lend(&mut sink, 3, |_| Ok("nothing put"));
    assert_eq!(kind(outcome), Kind::NotKept("nothing put"));
    assert_eq!(sink.len(), 3);
}

#[test]
fn a_slice_fill_that_gives_up_after_overfilling_stays_where_it_was() {
    let mut storage = [0; 4];
    let mut sink = &mut storage[..];
    let outcome = lend(&mut sink, 2, |lent| {
        BufMut::put_slice(lent.buffer().get_mut(), b"abcd");
        Err("gave up")
    });
    assert_eq!(kind(outcome), Kind::GaveUp("gave up"));
    assert_eq!(sink.len(), 4);
}

// Through the adapters: a loan that was not kept does not cost budget.

#[derive(Clone, Copy)]
enum Plan {
    /// Return without putting a byte.
    Nothing,
    /// Put at most the granted count of these bytes.
    Put(&'static [u8]),
    /// Raise the limit and put all these bytes, more than the grant.
    Overfill(&'static [u8]),
    GiveUp,
}
fn run_plan<B: BufMut>(lent: &mut LentBuffer<B>, plan: Plan) -> Result<&'static str, &'static str> {
    match plan {
        Plan::Nothing => Ok("nothing put"),
        Plan::Put(bytes) => {
            let count = bytes.len().min(lent.granted().get());
            lent.buffer().put_slice(&bytes[..count]);
            Ok("put")
        }
        Plan::Overfill(bytes) => {
            lent.buffer().set_limit(usize::MAX);
            lent.buffer().put_slice(bytes);
            Ok("overfilled")
        }
        Plan::GiveUp => Err("gave up"),
    }
}

/// Lend through an access, as an operation does.
async fn plan_through<A>(io: &mut A, wanted: usize, plan: Plan) -> Outcome
where
    A: ReceiverAccess<Family: WriteLent<(), Report = (), Error = !>>,
{
    poll_fn(|cx| {
        io.poll_access(cx, |view, cx| {
            <A::Family as WriteLent<()>>::poll_write_lent(view.into_pin(), cx, nz(wanted), |lent| {
                run_plan(lent, plan).map(|value| Filled { value, options: () })
            })
            .map(|result| result.unwrap())
        })
    })
    .await
}

#[call(sync)]
async fn plans_limited<F: WriteLent<(), Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
    budget: usize,
    wanted: usize,
    plans: &[Plan],
) -> (Vec<Kind>, usize) {
    let mut limited = WriteLimit::new(&mut *io, budget);
    let mut kinds = Vec::new();
    for plan in plans {
        kinds.push(kind(plan_through(&mut limited, wanted, *plan).await));
    }
    (kinds, limited.into_parts().1)
}

#[call(sync)]
async fn plans_skippable<F: WriteLent<(), Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
    wanted: usize,
    plans: &[Plan],
) -> Vec<Kind> {
    let mut by_reading = ReadSkip::new(&mut *io);
    let mut by_chunks = ReadChunkSkip::new(&mut by_reading);
    let mut kinds = Vec::new();
    for plan in plans {
        kinds.push(kind(plan_through(&mut by_chunks, wanted, *plan).await));
    }
    kinds
}

const BROKEN: [Plan; 3] = [Plan::Nothing, Plan::Overfill(b"abcde"), Plan::GiveUp];

/// Three loans the sink does not keep leave the whole budget; a fourth,
/// well-behaved one spends it.
fn assert_only_kept_loans_cost_budget(broken: (Vec<Kind>, usize), kept: (Vec<Kind>, usize)) {
    assert_eq!(
        broken,
        (
            vec![
                Kind::NotKept("nothing put"),
                Kind::NotKept("overfilled"),
                Kind::GaveUp("gave up")
            ],
            2
        )
    );
    assert_eq!(kept, (vec![Kind::Kept(2, "put")], 0));
}

#[test]
fn a_write_limit_over_a_vec_charges_only_the_loans_that_were_kept() {
    let mut sink = b"xy".to_vec();
    // The lent region is exactly the grant, so a fill cannot put more than
    // the grant; only loans that put nothing or give up are broken here.
    let broken = sink.sync_plans_limited_unpin(2, 8, &[Plan::Nothing, Plan::GiveUp]);
    assert_eq!(sink, b"xy");
    let kept = sink.sync_plans_limited_unpin(2, 8, &[Plan::Put(b"abcdef")]);
    assert_eq!(
        broken,
        (
            vec![Kind::NotKept("nothing put"), Kind::GaveUp("gave up")],
            2
        )
    );
    assert_eq!(kept, (vec![Kind::Kept(2, "put")], 0));
    assert_eq!(sink, b"xyab");
}

#[test]
fn a_write_limit_over_bytes_mut_charges_only_the_loans_that_were_kept() {
    let mut sink = BytesMut::from(&b"xy"[..]);
    let broken = sink.sync_plans_limited_unpin(2, 8, &BROKEN);
    assert_eq!(&sink[..], b"xy");
    let kept = sink.sync_plans_limited_unpin(2, 8, &[Plan::Put(b"abcdef")]);
    assert_only_kept_loans_cost_budget(broken, kept);
    assert_eq!(&sink[..], b"xyab");
}

#[test]
fn a_write_limit_over_a_slice_charges_only_the_loans_that_were_kept() {
    let mut storage = [0; 8];
    let mut sink = &mut storage[..];
    let broken = sink.sync_plans_limited_unpin(2, 8, &BROKEN);
    assert_eq!(sink.len(), 8);
    let kept = sink.sync_plans_limited_unpin(2, 8, &[Plan::Put(b"abcdef")]);
    assert_only_kept_loans_cost_budget(broken, kept);
    assert_eq!(sink.len(), 6);
    assert_eq!(&storage[..2], b"ab");
}

// A skip state composed on a sink forwards its lending, outcomes included.

#[test]
fn skip_states_composed_on_a_vec_forward_every_outcome() {
    let mut sink = b"xy".to_vec();
    let plans = [
        Plan::Put(b"ab"),
        Plan::Nothing,
        Plan::GiveUp,
        Plan::Put(b"c"),
    ];
    // The lent region is exactly the grant, so no fill can overfill it.
    let kinds = sink.sync_plans_skippable_unpin(4, &plans);
    assert_eq!(
        kinds,
        [
            Kind::Kept(2, "put"),
            Kind::NotKept("nothing put"),
            Kind::GaveUp("gave up"),
            Kind::Kept(1, "put"),
        ]
    );
    assert_eq!(sink, b"xyabc");
}

#[test]
fn skip_states_composed_on_a_slice_forward_none_and_every_outcome() {
    let mut storage = [0; 3];
    let mut sink = &mut storage[..];
    let plans = [
        Plan::Overfill(b"abc"),
        Plan::Put(b"ab"),
        Plan::Nothing,
        Plan::Put(b"cd"),
        // The slice is used up.
        Plan::Put(b"e"),
    ];
    let kinds = sink.sync_plans_skippable_unpin(2, &plans);
    assert_eq!(
        kinds,
        [
            Kind::NotKept("overfilled"),
            Kind::Kept(2, "put"),
            Kind::NotKept("nothing put"),
            Kind::Kept(1, "put"),
            Kind::Nothing,
        ]
    );
    assert_eq!(sink.len(), 0);
    assert_eq!(&storage[..3], b"abc");
}

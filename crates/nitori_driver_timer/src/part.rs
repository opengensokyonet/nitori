//! The part that keeps the timer table at the root.

use core::marker::PhantomData;

use nitori_driver::{
    call::{ClockOf, Composition, PriorityOf, RootOf},
    frunk_core::HList,
    structure::Share,
};

use crate::{FireTimers, TimerCallPriority, TimerTable};

/// The part that keeps the timer table, and the call that wakes what is
/// due, at the root of the composition `Q`, with its clock and its
/// priority. It is written in `Q`'s own list of parts, and has no
/// arguments; on every other kind it keeps nothing. The composition's
/// priority type gives the priority the call is woken at
/// ([`TimerCallPriority`]).
pub struct Part<Q>(PhantomData<fn() -> Q>);

impl<L, Q> nitori_driver::component::Part<L> for Part<Q>
where
    Q: Composition,
    PriorityOf<Q>: TimerCallPriority,
{
    type Args = ();
    type Shares = HList![
        Share<
            RootOf<Q>,
            HList![TimerTable<ClockOf<Q>, PriorityOf<Q>>],
            HList![FireTimers<ClockOf<Q>, PriorityOf<Q>>],
        >
    ];
}

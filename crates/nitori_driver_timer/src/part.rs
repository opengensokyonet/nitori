//! The part that keeps the timer table at the root.

use core::marker::PhantomData;

use nitori_driver::{
    call::{ClockOf, Composition, PriorityOf, RootOf},
    frunk_core::HList,
    id::Numbered,
    structure::Share,
};

use crate::{FireTimers, TimerCallPriority, TimerTable};

/// The part that keeps the timer table, and the call that wakes what is
/// due, at the root of the composition `Q`, with its clock and its
/// priority. It is written in `Q`'s own list of parts, and has no
/// arguments; on every other kind it keeps nothing. The composition's
/// priority type gives the priority the call is woken at
/// ([`TimerCallPriority`]).
///
/// It has a number ([`Numbered`]), the same whatever `Q` is, so that a
/// composition with it can have its own list of contributions and its
/// components can find other parts' arguments
/// ([`part_args`](nitori_driver::component::part_args)), which look for
/// every part by its number. A composition has one timer part, its own, and
/// `Part<Q>` is written only among the parts of `Q`, so the parts of two
/// compositions having one number does not matter; `Q` need not have a
/// number.
pub struct Part<Q>(PhantomData<fn() -> Q>);

mod number {
    use nitori_driver::id::Numbered;
    /// What the number of [`Part`](super::Part) is: one for every
    /// composition. Public only so that the number can be taken from it;
    /// not reachable from outside.
    #[derive(Numbered)]
    pub struct TimerPart;
}
impl<Q> Numbered for Part<Q> {
    type Number = <number::TimerPart as Numbered>::Number;
}

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

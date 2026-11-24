//! The part that keeps the timer table at the root.

use core::marker::PhantomData;

use frunk_core::hlist::{HList as HListTrait, HNil};
use nitori_clock::MonotonicClock;
use nitori_driver::{
    call::{ClockOf, Composition, PriorityOf, RootOf},
    frunk_core::{self, HList},
    id::{False, NumberEq, Numbered, True},
    structure::Share,
};

use crate::{FireTimers, TimerTable};

/// The part that keeps the timer table, and the call that wakes what is
/// due, at the root of the composition `Q`, with its clock and its
/// priority. It is written in `Q`'s own list of parts, and has no
/// arguments; on every other kind it keeps nothing.
pub struct Part<Q>(PhantomData<fn() -> Q>);

impl<L, Q> nitori_driver::component::Part<L> for Part<Q> {
    type Args = ();
}

impl<K, Q> Share<K> for Part<Q>
where
    Q: Composition<Root: Numbered>,
    K: Numbered,
    K::Number: NumberEq<<RootOf<Q> as Numbered>::Number, Out: AtRoot<ClockOf<Q>, PriorityOf<Q>>>,
{
    type Data = <IsRoot<K, Q> as AtRoot<ClockOf<Q>, PriorityOf<Q>>>::Data;
    type Calls = <IsRoot<K, Q> as AtRoot<ClockOf<Q>, PriorityOf<Q>>>::Calls;
}

/// Whether the kind `K` is the root of `Q`: [`True`] or [`False`].
type IsRoot<K, Q> = <<K as Numbered>::Number as NumberEq<<RootOf<Q> as Numbered>::Number>>::Out;

/// What [`Part`] keeps at a kind, by whether it is the root. Not for use
/// otherwise.
#[doc(hidden)]
pub trait AtRoot<C, P> {
    /// The components.
    type Data: HListTrait + 'static;
    /// The calls.
    type Calls: HListTrait + 'static;
}
impl<C: MonotonicClock + 'static, P: Ord + Copy + 'static> AtRoot<C, P> for True {
    type Data = HList![TimerTable<C, P>];
    type Calls = HList![FireTimers<C, P>];
}
impl<C, P> AtRoot<C, P> for False {
    type Data = HNil;
    type Calls = HNil;
}

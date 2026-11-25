//! The table of instants.

use alloc::vec::Vec;
use core::task::Waker;

use nitori_clock::MonotonicClock;
use nitori_driver::{
    component::{ArgsOf, Component, Part, PartList},
    frunk_core::hlist::HNil,
    id::Numbered,
    need::{ComponentWaker, Pos, View, WakeBox},
    structure::NodeKind,
};

/// The timer component: for each component waker at most one instant, and
/// the priority to wake it at.
///
/// `C` is the composition's clock and `P` its priority, and no others:
/// [`Part`](crate::Part) keeps one table at the root, and a need with
/// another clock or priority does not compile. Components take it as
/// `Mut<TimerTable<C, P>>`, which reaches the root from any node.
///
/// A component is woken when the clock's timer for its instant ends; a
/// clock's timer may end before its reading gets there, so the instant
/// the component's lending reads may still be earlier than the one it
/// set. Being woken is the instant having come: setting the same instant
/// again because the reading is earlier wakes it again at once.
///
/// An entry stays until its instant even when its node ends; the driver
/// then drops the wake, since the waker is of a node that is gone. A
/// component that sets instants clears its entry in its
/// [`end`](nitori_driver::component::Component::end), so that the table
/// does not keep entries of nodes that are gone.
pub struct TimerTable<C: MonotonicClock, P> {
    entries: Vec<(C::Instant, ComponentWaker, P)>,
    arm: Arm<C::Instant>,
    /// The timer call's standard waker, from its last poll.
    call: Option<Waker>,
}

/// What the timer call sleeps until, as far as `set` knows.
#[derive(Clone, Copy)]
pub(crate) enum Arm<I> {
    /// Nothing: the table was empty when the call last looked.
    Idle,
    /// The call sleeps until this instant.
    Until(I),
    /// A `set` came before what the call sleeps until, or into a table
    /// the call found empty; the call is woken to look again.
    Moved,
}

/// The number every timer table has, whatever its clock and priority: a
/// composition has one timer table, so the clock's and the priority's
/// types need no number of their own.
#[doc(hidden)]
#[derive(Numbered)]
pub struct TimerTableNumber;
impl<C: MonotonicClock, P> Numbered for TimerTable<C, P> {
    type Number = <TimerTableNumber as Numbered>::Number;
}

impl<C: MonotonicClock, P: Ord + Copy> TimerTable<C, P> {
    /// An empty table.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            arm: Arm::Idle,
            call: None,
        }
    }
    /// Wake `waker` at the priority `p` once `at` is reached; replaces
    /// what was set for `waker` before.
    ///
    /// When `at` is earlier than what the timer call sleeps until, the call
    /// is woken to sleep until `at`, at the priority
    /// [`TimerCallPriority::TIMER_CALL`](crate::TimerCallPriority::TIMER_CALL)
    /// gives; an entry of a priority above that waits for the call's turn
    /// at it before the call wakes the entry's component at its own.
    pub fn set(&mut self, at: C::Instant, waker: &ComponentWaker, p: P) {
        match self.entries.iter_mut().find(|(_, w, _)| w == waker) {
            Some(entry) => *entry = (at, *waker, p),
            None => self.entries.push((at, *waker, p)),
        }
        let moved = match self.arm {
            Arm::Idle => true,
            Arm::Until(armed) => at < armed,
            Arm::Moved => false,
        };
        if moved {
            self.arm = Arm::Moved;
            if let Some(call) = &self.call {
                call.wake_by_ref();
            }
        }
    }
    /// Forget what was set for `waker`.
    ///
    /// The timer call is not woken: when the instant it sleeps until was
    /// `waker`'s, it wakes then, finds nothing due, and sleeps until the
    /// next.
    pub fn clear(&mut self, waker: &ComponentWaker) {
        self.entries.retain(|(_, w, _)| w != waker);
    }
    /// The instant set for `waker`.
    pub fn get(&self, waker: &ComponentWaker) -> Option<C::Instant> {
        self.entries
            .iter()
            .find(|(_, w, _)| w == waker)
            .map(|(at, _, _)| *at)
    }

    /// Wake, at its priority, every entry due by `by`, and forget it.
    pub(crate) fn fire(&mut self, by: C::Instant, wakes: WakeBox<'_, P>) {
        self.entries.retain(|(at, w, p)| {
            let due = *at <= by;
            if due {
                wakes.wake(w, *p);
            }
            !due
        });
    }
    /// Arm for the earliest instant, `None` when the table is empty.
    pub(crate) fn arm(&mut self) -> Option<C::Instant> {
        let at = self.entries.iter().map(|(at, _, _)| *at).min();
        self.arm = match at {
            Some(at) => Arm::Until(at),
            None => Arm::Idle,
        };
        at
    }
    /// Keep the call's waker; whether a `set` moved the earliest instant
    /// since the call armed.
    pub(crate) fn register(&mut self, waker: &Waker) -> bool {
        match &mut self.call {
            Some(w) => w.clone_from(waker),
            None => self.call = Some(waker.clone()),
        }
        matches!(self.arm, Arm::Moved)
    }
}
impl<C: MonotonicClock, P: Ord + Copy> Default for TimerTable<C, P> {
    fn default() -> Self {
        Self::new()
    }
}

/// The table is built empty, by the timer part, at whichever kind it keeps
/// it at.
impl<Q, L, K, C, P> Component<crate::Part<Q>, L, K> for TimerTable<C, P>
where
    crate::Part<Q>: Part<L, Args = ()>,
    L: PartList<L>,
    K: NodeKind,
    C: MonotonicClock + 'static,
    P: Ord + Copy + 'static,
{
    fn build<Ps: Pos>(_: &mut K::Start, _: &(), _: &ArgsOf<L>, _: View<'_, HNil, Ps>) -> Self {
        Self::new()
    }
}

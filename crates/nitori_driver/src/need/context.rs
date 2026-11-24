//! The needs that read the lending itself: the clock the driver read for
//! it and the boxes requests are put in.
use alloc::vec::Vec;
use core::{cell::Cell, marker::PhantomData, time::Duration};

use nitori_clock::MonotonicClock;

use super::{ComponentWaker, Need, Pos};

/// The instant the driver read from its clock `C` for this lending, and
/// the addition of durations on that clock.
///
/// The lending's context holds it, so every view of one lending reads the
/// same instant.
///
/// Only a context that holds a [`LentClock`]`<C>` lends it: another clock
/// type does not compile.
pub struct Clock<C>(PhantomData<fn() -> C>);
impl<C: MonotonicClock + 'static> Need for Clock<C> {
    type View<'t, P: Pos> = &'t LentClock<C>;
}

/// What [`Clock`]`<C>` is lent as: the instant read for the lending and
/// the clock's addition.
pub struct LentClock<C: MonotonicClock> {
    clock: C,
    now: C::Instant,
}
impl<C: MonotonicClock> LentClock<C> {
    /// The clock `clock`, read at `now`.
    pub(crate) fn new(clock: C, now: C::Instant) -> Self {
        Self { clock, now }
    }
    /// Read again, at `now`.
    pub(crate) fn set_now(&mut self, now: C::Instant) {
        self.now = now;
    }
    /// The instant read for this lending.
    pub fn now(&self) -> C::Instant {
        self.now
    }
    /// `at` plus `by`, or `None` past the last instant the clock gives
    /// ([`MonotonicClock::checked_add`]).
    pub fn checked_add(&self, at: C::Instant, by: Duration) -> Option<C::Instant> {
        self.clock.checked_add(at, by)
    }
    /// A timer of the clock that ends at `at`
    /// ([`MonotonicClock::sleep_until`]).
    pub fn sleep_until(&self, at: C::Instant) -> C::Sleep {
        self.clock.sleep_until(at)
    }
}

/// Putting requests `R` in the box the lending's context holds for them;
/// the driver handles the requests of one lending in the order they were
/// put in, whatever their types.
///
/// Putting in only adds, so it overlaps no other need: a list may take
/// `Requests<R>` more than once, and every member of an
/// [`Under`](super::Under) may take it, all putting in the one box.
///
/// ```
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends_in!(
///     HList![RequestList<Ping>, RequestList<Close>],
///     LinkPath,
///     HList![Mut<LinkCore>, Requests<Ping>, Requests<Close>, At<SentRecords, HList![Requests<Ping>]>]
/// );
/// lends_in!(
///     HList![RequestList<Ping>],
///     Conn,
///     HList![Under<ConnCore, At<LinkCore, HList![Mut<LinkCore>, Requests<Ping>]>>, Requests<Ping>]
/// );
/// ```
///
/// A context without its box does not compile:
///
/// ```compile_fail,E0277
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends_in!(HList![RequestList<Close>], LinkPath, HList![Requests<Ping>]);
/// ```
pub struct Requests<R>(PhantomData<fn() -> R>);
impl<R: 'static> Need for Requests<R> {
    type View<'t, P: Pos> = RequestBox<'t, R>;
}

/// What [`Requests`]`<R>` is lent as: the box the requests are put in.
///
/// A handle, copied to every need that takes it; each request put in is
/// numbered from the one order of the lending, so the requests of all
/// types keep the order they were put in.
pub struct RequestBox<'t, R> {
    requests: &'t Cell<Vec<(u64, R)>>,
    order: &'t Cell<u64>,
}
impl<R> Clone for RequestBox<'_, R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R> Copy for RequestBox<'_, R> {}
impl<'t, R> RequestBox<'t, R> {
    /// The box of `requests`, numbered from `order`.
    pub(crate) fn new(requests: &'t Cell<Vec<(u64, R)>>, order: &'t Cell<u64>) -> Self {
        Self { requests, order }
    }
    /// Put `request` in, after every request put in before it in this
    /// lending.
    pub fn push(&self, request: R) {
        let at = self.order.get();
        self.order.set(at + 1);
        let mut requests = self.requests.take();
        requests.push((at, request));
        self.requests.set(requests);
    }
}

/// Waking components at a priority `P`: the wakes put in during a
/// lending, which the driver ranks in its ready queue after the lending's
/// requests.
///
/// `P` is the composition's priority; a context without a list for it
/// does not compile. Waking only adds, so it overlaps no other need: a list
/// may take `Wakes<P>` more than once, and every member of an
/// [`Under`](super::Under) may take it, all waking through the one list.
///
/// ```
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends_in!(
///     HList![WakeList<u8>],
///     LinkPath,
///     HList![
///         Mut<LinkCore>,
///         WakerOf<Planner>,
///         Wakes<u8>,
///         Under<ConnCore, HList![WakerOf<LinkCore>, Wakes<u8>]>,
///         Under<ConnCore, OneAtATime<HList![Mut<Planner>, Wakes<u8>]>>
///     ]
/// );
/// ```
///
/// Another priority than the context's does not compile:
///
/// ```compile_fail,E0277
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends_in!(HList![WakeList<u8>], LinkPath, HList![Wakes<u16>]);
/// ```
pub struct Wakes<P>(PhantomData<fn() -> P>);
impl<P: 'static> Need for Wakes<P> {
    type View<'t, Q: Pos> = WakeBox<'t, P>;
}

/// What [`Wakes`]`<P>` is lent as: the list the wakes are put in.
///
/// A handle, copied to every need that takes it.
pub struct WakeBox<'t, P> {
    wakes: &'t Cell<Vec<(ComponentWaker, P)>>,
}
impl<P> Clone for WakeBox<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<P> Copy for WakeBox<'_, P> {}
impl<'t, P> WakeBox<'t, P> {
    /// The box of `wakes`.
    pub(crate) fn new(wakes: &'t Cell<Vec<(ComponentWaker, P)>>) -> Self {
        Self { wakes }
    }
    /// Wake the component of `waker` at the priority `p`. Woken again
    /// before it is handled, it is handled once, at the highest priority
    /// it was woken at.
    pub fn wake(&self, waker: &ComponentWaker, p: P) {
        let mut wakes = self.wakes.take();
        wakes.push((*waker, p));
        self.wakes.set(wakes);
    }
}

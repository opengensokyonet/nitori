//! The needs that read the lending itself: the clock the driver read for
//! it and the boxes requests are put in.
use alloc::vec::Vec;
use core::{marker::PhantomData, time::Duration};

use nitori_clock::MonotonicClock;

use super::{Need, Pos};

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
    /// The instant read for this lending.
    pub fn now(&self) -> C::Instant {
        self.now
    }
    /// `at` plus `by`, or `None` past the last instant the clock gives
    /// ([`MonotonicClock::checked_add`]).
    pub fn checked_add(&self, at: C::Instant, by: Duration) -> Option<C::Instant> {
        self.clock.checked_add(at, by)
    }
}

/// Putting requests `R` in the box the lending's context holds for them;
/// the driver handles them in the order they were put in.
///
/// Putting in only adds, so it does not overlap any other need; a list
/// takes one `Requests<R>` for each type of request. Members of an
/// [`Under`](super::Under) lent all at once would share the one box, so
/// there it is taken [`OneAtATime`](super::OneAtATime).
///
/// ```
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends_in!(
///     HList![RequestBox<Ping>, RequestBox<Close>],
///     LinkPath,
///     HList![Mut<LinkCore>, Requests<Ping>, Requests<Close>]
/// );
/// lends_in!(
///     HList![RequestBox<Ping>],
///     Conn,
///     HList![Under<ConnCore, OneAtATime<At<LinkCore, HList![Mut<LinkCore>, Requests<Ping>]>>>]
/// );
/// ```
///
/// The same type of request twice in one list does not compile, nor does
/// a context without its box:
///
/// ```compile_fail,E0277
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends_in!(
///     HList![RequestBox<Ping>],
///     LinkPath,
///     HList![Requests<Ping>, At<SentRecords, HList![Requests<Ping>]>]
/// );
/// ```
///
/// ```compile_fail,E0277
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends_in!(HList![RequestBox<Close>], LinkPath, HList![Requests<Ping>]);
/// ```
///
/// and neither do members lent all at once that each put requests in:
///
/// ```compile_fail,E0277
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends_in!(
///     HList![RequestBox<Ping>],
///     Conn,
///     HList![Under<ConnCore, At<LinkCore, HList![Mut<LinkCore>, Requests<Ping>]>>]
/// );
/// ```
pub struct Requests<R>(PhantomData<fn() -> R>);
impl<R: 'static> Need for Requests<R> {
    type View<'t, P: Pos> = &'t mut RequestBox<R>;
}

/// What [`Requests`]`<R>` is lent as: the box the requests are put in.
pub struct RequestBox<R> {
    requests: Vec<R>,
}
impl<R> RequestBox<R> {
    /// An empty box.
    pub(crate) fn new() -> Self {
        Self {
            requests: Vec::new(),
        }
    }
    /// Put `request` in, after those put in before it.
    pub fn push(&mut self, request: R) {
        self.requests.push(request);
    }
    /// The requests put in, in order, leaving the box empty.
    pub(crate) fn take(&mut self) -> Vec<R> {
        core::mem::take(&mut self.requests)
    }
}

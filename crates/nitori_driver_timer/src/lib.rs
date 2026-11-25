//! The timer component of [`nitori_driver`], and the part that keeps it
//! at the root.
//!
//! A component that is to be woken at an instant sets it in the
//! [`TimerTable`] with its [`ComponentWaker`](nitori_driver::need::ComponentWaker) and the priority to wake
//! it at: [`set(at, &waker, p)`](TimerTable::set). One waker has at most
//! one instant, and setting it again replaces it; a component that waits
//! for several instants sets the earliest. An instant wakes once: when it
//! has come, its entry is gone. [`clear(&waker)`](TimerTable::clear)
//! forgets it before, and a component that sets instants clears its entry
//! when its node ends.
//!
//! The table is an ordinary component. A call next to it, at the root,
//! sleeps on the composition's clock until the earliest instant, wakes
//! every component whose instant has come at the priority it was set
//! with, and sleeps again; a `set` that moves the earliest instant earlier
//! wakes the call to sleep anew, at the priority the composition's
//! priority type gives it ([`TimerCallPriority`]). The driver knows
//! nothing of timers.
//!
//! [`Part<Q>`](Part) keeps the table and the call at the root of the
//! composition `Q`, with its clock and its priority; it is written in the
//! composition's own list of parts:
//!
//! ```
//! #![feature(impl_trait_in_assoc_type)]
//! use core::{pin::Pin, task::{Context, Waker}, time::Duration};
//!
//! use nitori_driver::{
//!     component::{ArgsOf, Component, Part, PartList},
//!     composition,
//!     driver::Driver,
//!     frunk_core::{HList, hlist, hlist::HNil, hlist_pat},
//!     id::Numbered,
//!     need::{Clock, Mut, Pos, View, WakerOf},
//!     structure::{KeyAddress, NodeKind, Share},
//! };
//! use nitori_driver_timer::TimerTable;
//! # #[derive(Clone)]
//! # struct Stopped;
//! # impl nitori_clock::MonotonicClock for Stopped {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//!
//! #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
//! struct Level(u8);
//! impl nitori_driver_timer::TimerCallPriority for Level {
//!     const TIMER_CALL: Self = Level(2);
//! }
//!
//! #[derive(Numbered)]
//! struct Conn;
//! impl NodeKind for Conn {
//!     type Children = HList![];
//! }
//! /// A component that waits for an instant.
//! #[derive(Numbered)]
//! struct Idle;
//! impl<L: PartList<L>> Component<Base, L, Conn> for Idle {
//!     fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
//!         Idle
//!     }
//! }
//! struct Base;
//! impl<L> Part<L> for Base {
//!     type Args = ();
//!     type Shares = HList![Share<Conn, HList![Idle]>];
//! }
//!
//! struct Q;
//! composition!(impl for Q: Conn, HList![Base, nitori_driver_timer::Part<Q>], Stopped, Level);
//!
//! type Table = TimerTable<Stopped, Level>;
//! let mut driver = Driver::<Q>::new(Stopped, (), hlist![(), ()], Level(0));
//! driver
//!     .operate::<Conn, HList![Mut<Table>, Clock<Stopped>, WakerOf<Idle>], _, _, _>(
//!         &KeyAddress::here(),
//!         |hlist_pat![table, clock, waker]| {
//!             let at = clock.checked_add(clock.now(), Duration::from_secs(30)).unwrap();
//!             table.set(at, &waker, Level(1));
//!         },
//!     )
//!     .unwrap()
//!     .unwrap();
//! let _ = Pin::new(&mut driver).poll(&mut Context::from_waker(Waker::noop()));
//! ```
#![no_std]
#![forbid(unsafe_code)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

mod fire;
mod part;
mod table;

pub use fire::{FireTimers, TimerCallPriority};
pub use part::Part;
pub use table::TimerTable;
#[doc(hidden)]
pub use table::TimerTableNumber;

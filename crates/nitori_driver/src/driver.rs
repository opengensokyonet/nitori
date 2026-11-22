//! The driver: the tree, its calls and its hooks, as one future.
//!
//! A [`Driver`] keeps the root of a [`DriverComposition`]'s tree, every
//! call of every node and the driver's own tables, and runs as a future
//! that ends when the root ends. Internally a node has a number; the
//! places of the tree and the call slots are reached by their numbers
//! ([`walk`]), so no function's type names a whole pack.
//!
//! A poll:
//!
//! 1. takes the nodes marked since the last poll (see waking, below) and runs the
//!    components that act before the calls and were woken, the nodes
//!    above first;
//! 2. takes the nodes marked by then, and polls every marked call of each
//!    once, in the order the nodes were first marked;
//! 3. runs the deadlines that came;
//! 4. sleeps until the earliest deadline.
//!
//! After every lend (a call's poll, a hook) the driver reads again the
//! deadlines of the components of the types the lend took exclusive and
//! hands the events it published to their subscribers, round after round
//! until a round publishes nothing ([`event`](crate::event)). A request a
//! hook returns or a call yields ([`request`](crate::request)) is handled
//! right after its lend, by the node it is for, before any other call is
//! polled; what its edits started and ended is handed on with the lend's
//! events.
//!
//! Waking needs no lock: a waker sets its position's bit in its node's
//! head; a node whose bits were clear goes on a ready stack linked through
//! the heads by node number, and the outer waker is woken the first time
//! since the poll began. A waker holds the outer waker of the poll it was
//! made in; polled with another outer waker, the driver makes new wakers
//! and polls every call once. A node's number goes to another node only
//! once no waker of the old one is alive and it is off the stack, so a
//! waker of a node that ended wakes nothing.
//!
//! Nodes are added and removed through [`Driver::add`], [`Driver::remove`]
//! and [`Driver::end`] from outside, and through the edit handle of a
//! request from inside: a node is built from the view of its parent, its
//! calls start woken, its deadlines are read and [`NodeStarted`] is
//! published; a node ends after the nodes below it (a node others link to
//! after its siblings' subtrees), [`NodeEnding`] reaching the nodes above
//! it, its calls dropped, its components' end hooks run while it is still
//! in the tree.
//!
//! A driver dropped before the root ended tells every component that it
//! was dropped ([`Hooks::aborted`](crate::hooks::Hooks::aborted)), the
//! nodes above first, and ends every node. After a panic through a poll
//! only the root's components are told, their remains are kept, and no
//! node ends.
//!
//!
//! A layer whose deadline is a count at the root, and which hears its own
//! start:
//!
//! ```
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! #[derive(Default)]
//! struct Due {
//!     started: bool,
//! }
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Layer> for Due {
//!     type Needs = HList![At<Kind<Root>, Here, Ref<Count>>];
//!     type BuildNeeds = HList![At<Up<0>, Here, Ref<Count>>];
//!     type Subscribes = HList![Hear<NodeStarted<Layer>>];
//!     type Timing = HasDeadline;
//!     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Layer>>>(
//!         parents: BuildView<'_, Self, Part1, L, Layer, KP>,
//!         _: &ArgsOf<L>,
//!     ) -> Self {
//!         let hlist_pat![count] = parents;
//!         assert_eq!(count.0, 0);
//!         Due::default()
//!     }
//!     fn on_events<KP: HookKeys<Self, Part1, L, Layer>>(
//!         _: nitori_driver::event::Triggered<'_>,
//!         view: HookView<'_, Self, Part1, L, Layer, KP>,
//!     ) -> Option<Self::Returns> {
//!         view.head.started = true;
//!         None
//!     }
//! }
//! impl<L: PartList<L> + 'static> Timed<Part1, L, Layer> for Due {
//!     type Instant = Duration;
//!     fn deadline<KP: HookKeys<Self, Part1, L, Layer>>(
//!         view: &HookView<'_, Self, Part1, L, Layer, KP>,
//!     ) -> Option<Duration> {
//!         Some(Duration::from_secs(view.tail.head.0 + 1))
//!     }
//!     fn expire<KP: HookKeys<Self, Part1, L, Layer>>(
//!         _: HookView<'_, Self, Part1, L, Layer, KP>,
//!         _: Duration,
//!     ) -> Option<Self::Returns> {
//!         None
//!     }
//! }
//! impl Share<Layer> for Part1 {
//!     type Data = HList![Due];
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//!
//! let mut driver = Driver::<Q>::new(Clock, frunk_core::hlist![()]).unwrap();
//! let layer = driver.add::<Layer, _, _>(End, ()).unwrap();
//! assert!(Pin::new(&mut driver).poll(&mut Context::from_waker(Waker::noop())).is_pending());
//! use nitori_driver::structure::NodePath;
//! assert!(layer.node(driver.root().unwrap()).unwrap().data.head.head.started);
//! driver.end(());
//! assert!(Pin::new(&mut driver).poll(&mut Context::from_waker(Waker::noop())).is_ready());
//! ```
//!
//! # What does not compile
//!
//! A deadline whose instant is not the clock's:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! #[derive(Default)]
//! struct Due;
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Layer> for Due {
//!     type Timing = HasDeadline;
//!     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Layer>>>(
//!         _: BuildView<'_, Self, Part1, L, Layer, KP>,
//!         _: &ArgsOf<L>,
//!     ) -> Self {
//!         Due
//!     }
//! }
//! impl<L: PartList<L> + 'static> Timed<Part1, L, Layer> for Due {
//!     type Instant = u64;
//!     fn deadline<KP: HookKeys<Self, Part1, L, Layer>>(
//!         _: &HookView<'_, Self, Part1, L, Layer, KP>,
//!     ) -> Option<u64> {
//!         None
//!     }
//!     fn expire<KP: HookKeys<Self, Part1, L, Layer>>(
//!         _: HookView<'_, Self, Part1, L, Layer, KP>,
//!         _: u64,
//!     ) -> Option<Self::Returns> {
//!         None
//!     }
//! }
//! impl Share<Layer> for Part1 {
//!     type Data = HList![Due];
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
//!
//! A hook's need not on the structure (the layer has no `Count`):
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! #[derive(Default)]
//! struct Reads;
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Layer> for Reads {
//!     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
//!     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Layer>>>(
//!         _: BuildView<'_, Self, Part1, L, Layer, KP>,
//!         _: &ArgsOf<L>,
//!     ) -> Self {
//!         Reads
//!     }
//! }
//! impl Share<Layer> for Part1 {
//!     type Data = HList![Reads];
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
//!
//! A component of the root that reads a parent when it is built:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     type BuildNeeds = HList![At<Up<0>, Here, Ref<Count>>];
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! impl Share<Layer> for Part1 {
//!     type Data = HNil;
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
//!
//! A subscription whose anchor is not on the way up:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! #[derive(Default)]
//! struct Hears;
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Layer> for Hears {
//!     type Subscribes = HList![HearAt<NodeStarted<Layer>, Kind<Other>, Here>];
//!     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Layer>>>(
//!         _: BuildView<'_, Self, Part1, L, Layer, KP>,
//!         _: &ArgsOf<L>,
//!     ) -> Self {
//!         Hears
//!     }
//! }
//! impl Share<Layer> for Part1 {
//!     type Data = HList![Hears];
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
//!
//! [`NodeStarted`]: crate::event::NodeStarted
//! [`NodeEnding`]: crate::event::NodeEnding
mod place;
mod wake;
pub mod walk;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec,
    vec::Vec,
};
use core::{
    any::TypeId,
    fmt,
    future::Future,
    pin::Pin,
    sync::atomic::Ordering::SeqCst,
    task::{Context, Poll, Waker},
};

use frunk_core::indices::Here as I0;
use nitori_clock::MonotonicClock;

pub use place::{
    AddressOf, BuildAt, ChildPath, ChildPlaces, ClockOf, CompInfo, CompLend, CompPositions, Detach,
    DriverComposition, FindPlace, InstantOf, NoPlace, NoPlacePos, NthPlace, ParentOf, ParentPlace,
    PartsOps, Place, PlaceAt, PlaceDesc, PlaceInfo, PlaceNth, PlaceOps, PlacePack, PlacePositions,
    PlacePositionsAt, PlaceShape, PlacesOf, PlacesUnder, ProveComp, ProveParts, ProvePlace,
    ProvePlaces, RequestsOf, RootNode, SelfParts, ShareOps, TargetOf, prove_places,
};
use wake::{NodeHead, PositionWaker, Shared};
use walk::{CallWalk, Cursor, Number, PlaceVisit, PlaceWalk, SlotInfo, SlotStore, SlotVisit};

use crate::{
    call::{CallStep, LendOne, PartsOf, RootOf, SlotCall, SlotDesc, SlotYield},
    event::{Outbox, Scope, Triggered},
    hooks::{AbortCause, ArgsOf, EndReasonOf},
    request::{
        RequestSum,
        sealed::{EditOps, FindNode},
    },
    structure::{
        Address, Append, ChildAddresses, ChildAt, ChildList, ChildOfKind, End, FromAddress,
        HandleAt, KeyAddress, KeyAt, KindFor, LinkList, MoveError, NodeKind, NodePath,
        ParentChildren, PathFrom, RemainsOf, resolve_node,
    },
};

// ---------------------------------------------------------------- errors

/// Why a composition cannot be driven.
#[derive(Debug, PartialEq, Eq)]
pub enum CompositionError {
    /// A place has more than 64 calls and components that act before the
    /// calls together: a node marks each in one bit of a word.
    TooManyPositions {
        /// The place's number, in tree order.
        place: usize,
    },
    /// A component's handler runs for more than 64 events: it is told
    /// them in one bit each.
    TooManySubscriptions {
        /// The place's number, in tree order.
        place: usize,
        /// The component's number at the place.
        component: usize,
    },
}
impl fmt::Display for CompositionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyPositions { place } => write!(
                f,
                "the place {place} has more than 64 calls and components that act before the calls"
            ),
            Self::TooManySubscriptions { place, component } => write!(
                f,
                "the component {component} of the place {place} handles more than 64 events"
            ),
        }
    }
}
impl core::error::Error for CompositionError {}

/// Why a node was not added.
#[derive(Debug, PartialEq, Eq)]
pub enum AddError {
    /// The root ended.
    Ended,
    /// No node at the parent's path.
    NoParent,
    /// The parent has a child of this kind under the same key.
    Occupied,
}
impl fmt::Display for AddError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ended => "the root ended",
            Self::NoParent => "no node at the parent's path",
            Self::Occupied => "the parent has a child of this kind under this key",
        })
    }
}
impl core::error::Error for AddError {}

// ---------------------------------------------------------------- the driver's tables

/// What a hook or a call is, among a node's positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    /// The component at this number of the place.
    Comp(usize),
    /// The call at this number of the place.
    Call(usize),
}

/// A place as the driver runs it.
struct PlaceRt {
    info: PlaceInfo,
    /// The call slots, by their number among the place's calls.
    calls: Vec<usize>,
    /// The components that act before the calls; their bits follow the
    /// calls'.
    before: Vec<usize>,
    /// For each component, the events its handler runs for, in order.
    handled: Vec<Vec<TypeId>>,
    /// Whether a kind of the structure links to this place's kind.
    link_target: bool,
    /// The places of its children, by their position among the kind's
    /// children.
    children: Vec<Option<usize>>,
}
impl PlaceRt {
    fn positions(&self) -> usize {
        self.calls.len() + self.before.len()
    }
    fn calls_mask(&self) -> u64 {
        (1u64 << self.calls.len()) - 1
    }
    fn before_mask(&self) -> u64 {
        ((1u64 << self.before.len()) - 1) << self.calls.len()
    }
}

/// Who hears an event.
#[derive(Clone, Copy, Debug)]
struct Subscriber {
    place: usize,
    target: Target,
    /// The bit among the events the handler runs for; `None` for a
    /// deadline only, and for a call.
    bit: Option<u32>,
    scope: Scope,
}

/// A node the driver keeps.
struct NodeEntry<A> {
    head: Arc<NodeHead>,
    /// Told apart from a node that had its number before.
    serial: u64,
    place: usize,
    addr: A,
    parent: Option<usize>,
    children: Vec<usize>,
    /// The waker of each position, with the number of the outer waker it
    /// was made with.
    wakers: Vec<Option<(u64, Waker)>>,
}
enum NodeSlot<A> {
    Free,
    Live(NodeEntry<A>),
    /// A node that ended, whose number waits until no waker refers to it.
    Retired(Arc<NodeHead>),
}
impl<A> NodeSlot<A> {
    fn head(&self) -> Option<&NodeHead> {
        match self {
            Self::Free => None,
            Self::Live(n) => Some(&n.head),
            Self::Retired(h) => Some(h),
        }
    }
    fn live(&self) -> Option<&NodeEntry<A>> {
        match self {
            Self::Live(n) => Some(n),
            _ => None,
        }
    }
}

/// What one round of events handles at one position.
#[derive(Clone, Copy, Debug)]
struct Hit {
    node: usize,
    serial: u64,
    target: Target,
    /// The events that reached it, by their bits; none for a deadline to
    /// read again.
    events: u64,
}

/// The sleep until the earliest deadline, with that deadline.
type Sleeping<Q> = (
    InstantOf<Q>,
    Pin<Box<<ClockOf<Q> as MonotonicClock>::Sleep>>,
);
/// The stores of every call slot of `Q`.
pub type CallStoresOf<Q> = <Cursor<I0> as CallWalk<Q>>::Store;

/// The driver of a composition `Q`: the tree, its calls and its hooks, as
/// one future that ends when the root ends.
pub struct Driver<Q: DriverComposition>
where
    Cursor<I0>: CallWalk<Q>,
{
    root: Option<RootNode<Q>>,
    args: ArgsOf<PartsOf<Q>>,
    calls: CallStoresOf<Q>,
    nodes: Vec<NodeSlot<AddressOf<Q>>>,
    places: Vec<PlaceRt>,
    slots: Vec<SlotInfo>,
    subscribers: Vec<(TypeId, Subscriber)>,
    shared: Arc<Shared>,
    outer: Option<Waker>,
    epoch: u64,
    clock: ClockOf<Q>,
    sleep: Option<Sleeping<Q>>,
    timers: BTreeSet<(InstantOf<Q>, usize, usize)>,
    deadlines: BTreeMap<(usize, usize), InstantOf<Q>>,
    outbox: Outbox,
    /// What the edits of the request being handled reach.
    pending: Vec<Hit>,
    /// The serial of the last node registered.
    serial: u64,
    /// The instant read when the driver's code was last entered.
    now: InstantOf<Q>,
    remains: Option<Q::Remains>,
    ended: bool,
    /// The driver's poll runs: still set when it is dropped or polled
    /// again, a panic unwound through it.
    running: bool,
    /// What a drop before the end runs; taken where the driver is made,
    /// which has the bounds it needs.
    on_drop: fn(&mut Self),
}

impl<Q: DriverComposition> Drop for Driver<Q>
where
    Cursor<I0>: CallWalk<Q>,
{
    fn drop(&mut self) {
        if !self.ended {
            (self.on_drop)(self);
        }
    }
}
// Nothing in the driver is pinned structurally: the calls and the sleep
// are boxed.
impl<Q: DriverComposition> Unpin for Driver<Q> where Cursor<I0>: CallWalk<Q> {}

// ---------------------------------------------------------------- visits

struct Deadline<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    c: usize,
    outbox: &'r Outbox,
}
impl<Q: DriverComposition> PlaceVisit<Q> for Deadline<'_, Q> {
    type Out = Option<InstantOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::deadline(self.root, self.at, self.c, self.outbox)
    }
}
struct Expire<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    c: usize,
    now: InstantOf<Q>,
    outbox: &'r Outbox,
}
impl<Q: DriverComposition> PlaceVisit<Q> for Expire<'_, Q> {
    type Out = Option<RequestsOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::expire(self.root, self.at, self.c, self.now, self.outbox)
    }
}
struct OnEvents<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    c: usize,
    events: Triggered<'r>,
    outbox: &'r Outbox,
}
impl<Q: DriverComposition> PlaceVisit<Q> for OnEvents<'_, Q> {
    type Out = Option<RequestsOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::on_events(self.root, self.at, self.c, self.events, self.outbox)
    }
}
struct BeforeCalls<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    c: usize,
    waker: &'r Waker,
    outbox: &'r Outbox,
}
impl<Q: DriverComposition> PlaceVisit<Q> for BeforeCalls<'_, Q> {
    type Out = Option<RequestsOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::before_calls(self.root, self.at, self.c, self.waker, self.outbox)
    }
}
struct Aborted<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    cause: AbortCause,
    outbox: &'r Outbox,
}
impl<Q: DriverComposition> PlaceVisit<Q> for Aborted<'_, Q> {
    type Out = ();
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) {
        Pos::aborted(self.root, self.at, self.cause, self.outbox)
    }
}
struct EndDefault<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    outbox: &'r Outbox,
}
impl<Q: DriverComposition> PlaceVisit<Q> for EndDefault<'_, Q> {
    type Out = ();
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) {
        Pos::end_default(self.root, self.at, self.outbox)
    }
}
struct DetachAt<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for DetachAt<'_, Q> {
    type Out = bool;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> bool {
        Pos::detach(self.root, self.at)
    }
}

struct AddAt<'r, Q: DriverComposition, KK> {
    root: &'r mut RootNode<Q>,
    parent: &'r AddressOf<Q>,
    key: KK,
    args: &'r ArgsOf<PartsOf<Q>>,
    outbox: &'r Outbox,
}
impl<Q: DriverComposition, KK: 'static> PlaceVisit<Q> for AddAt<'_, Q, KK> {
    type Out = Option<AddressOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::add(self.root, self.parent, self.key, self.args, self.outbox)
    }
}
struct FindChild<'r, Q: DriverComposition, KK> {
    root: &'r RootNode<Q>,
    parent: &'r AddressOf<Q>,
    key: &'r KK,
}
impl<Q: DriverComposition, KK: 'static> PlaceVisit<Q> for FindChild<'_, Q, KK> {
    type Out = Option<AddressOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::find_child(self.root, self.parent, self.key)
    }
}
struct MoveUnder<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    to: &'r AddressOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for MoveUnder<'_, Q> {
    type Out = Result<AddressOf<Q>, MoveError>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::move_under(self.root, self.at, self.to)
    }
}
struct EndWith<'r, Q: DriverComposition, RR> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    reason: &'r RR,
    outbox: &'r Outbox,
}
impl<Q: DriverComposition, RR: 'static> PlaceVisit<Q> for EndWith<'_, Q, RR> {
    type Out = ();
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) {
        Pos::end_with(self.root, self.at, self.reason, self.outbox)
    }
}

struct Start<'r, Q: DriverComposition> {
    node: usize,
    at: &'r AddressOf<Q>,
}
impl<Q: DriverComposition> SlotVisit<Q> for Start<'_, Q> {
    type Out = ();
    fn visit<S, Pos>(self, store: &mut SlotStore<Pos::Running, S::Yield, AddressOf<Q>>)
    where
        S: SlotDesc<At: FromAddress<RootOf<Q>> + 'static> + SlotYield<Q>,
        Pos: SlotCall<Q, S> + for<'a> LendOne<'a, Q, S>,
    {
        let Some(at) = S::At::from_address(self.at) else {
            return;
        };
        if store.calls.len() <= self.node {
            store.calls.resize_with(self.node + 1, || None);
        }
        store.calls[self.node] = Some(Box::pin(Pos::start(&at)));
    }
}
struct Stop {
    node: usize,
}
impl<Q: DriverComposition> SlotVisit<Q> for Stop {
    type Out = ();
    fn visit<S, Pos>(self, store: &mut SlotStore<Pos::Running, S::Yield, AddressOf<Q>>)
    where
        S: SlotDesc<At: FromAddress<RootOf<Q>> + 'static> + SlotYield<Q>,
        Pos: SlotCall<Q, S> + for<'a> LendOne<'a, Q, S>,
    {
        if let Some(call) = store.calls.get_mut(self.node) {
            *call = None;
        }
    }
}
/// What polling a call came to, as the driver goes on from it.
enum Polled {
    /// Waits, or is not there.
    Pending,
    /// Over, or no longer needed: dropped.
    Dropped,
    /// Yielded and goes on: it is polled again.
    Yielded,
}
struct PollCall<'r, 'c, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    node: usize,
    at: &'r AddressOf<Q>,
    outbox: &'r Outbox,
    cx: &'r mut Context<'c>,
}
impl<Q: DriverComposition> SlotVisit<Q> for PollCall<'_, '_, Q> {
    type Out = Polled;
    fn visit<S, Pos>(self, store: &mut SlotStore<Pos::Running, S::Yield, AddressOf<Q>>) -> Polled
    where
        S: SlotDesc<At: FromAddress<RootOf<Q>> + 'static> + SlotYield<Q>,
        Pos: SlotCall<Q, S> + for<'a> LendOne<'a, Q, S>,
    {
        let Some(at) = S::At::from_address(self.at) else {
            return Polled::Pending;
        };
        let Some(Some(call)) = store.calls.get_mut(self.node) else {
            return Polled::Pending;
        };
        match Pos::poll(self.root, at, self.outbox, call.as_mut(), self.cx) {
            Ok(CallStep::Pending) | Err(_) => Polled::Pending,
            Ok(CallStep::Finished | CallStep::NoLongerNeeded) => {
                store.calls[self.node] = None;
                Polled::Dropped
            }
            Ok(CallStep::Yielded(item)) => {
                store.yields.push(item);
                Polled::Yielded
            }
        }
    }
}

// ---------------------------------------------------------------- the driver

impl<Q: DriverComposition> Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
    /// A driver whose root is built from `args`; it reads and sleeps on
    /// `clock`. The root's calls start woken.
    pub fn new(clock: ClockOf<Q>, args: ArgsOf<PartsOf<Q>>) -> Result<Self, CompositionError> {
        let mut infos = Vec::new();
        <Cursor<I0> as PlaceWalk<Q>>::info(&mut infos);
        let mut slots = Vec::new();
        <Cursor<I0> as CallWalk<Q>>::info(&mut slots);
        let mut targets = Vec::new();
        <<RootOf<Q> as NodeKind>::Linked as LinkList<RootOf<Q>>>::targets(&mut targets);
        <<RootOf<Q> as NodeKind>::Children as ChildList>::link_targets(&mut targets);
        let mut places = Vec::new();
        for (p, info) in infos.into_iter().enumerate() {
            let calls: Vec<usize> = (0..slots.len())
                .filter(|&s| slots[s].place == info.path)
                .collect();
            let before: Vec<usize> = (0..info.comps.len())
                .filter(|&c| info.comps[c].acts_before_calls)
                .collect();
            if calls.len() + before.len() > 64 {
                return Err(CompositionError::TooManyPositions { place: p });
            }
            let mut handled = Vec::new();
            for (c, comp) in info.comps.iter().enumerate() {
                let h: Vec<TypeId> = comp
                    .subscriptions
                    .iter()
                    .filter(|s| s.handled)
                    .map(|s| s.event)
                    .collect();
                if h.len() > 64 {
                    return Err(CompositionError::TooManySubscriptions {
                        place: p,
                        component: c,
                    });
                }
                handled.push(h);
            }
            places.push(PlaceRt {
                link_target: targets.contains(&info.kind),
                calls,
                before,
                handled,
                children: Vec::new(),
                info,
            });
        }
        for q in 0..places.len() {
            let (Some(parent), Some(i)) = (places[q].info.parent, places[q].info.index) else {
                continue;
            };
            if let Some(p) = places.iter().position(|p| p.info.path == parent) {
                let children = &mut places[p].children;
                if children.len() <= i {
                    children.resize(i + 1, None);
                }
                children[i] = Some(q);
            }
        }
        let mut subscribers = Vec::new();
        for (p, place) in places.iter().enumerate() {
            for (c, comp) in place.info.comps.iter().enumerate() {
                let mut bit = 0;
                for s in &comp.subscriptions {
                    let this = s.handled.then(|| {
                        bit += 1;
                        bit - 1
                    });
                    subscribers.push((
                        s.event,
                        Subscriber {
                            place: p,
                            target: Target::Comp(c),
                            bit: this,
                            scope: s.scope,
                        },
                    ));
                }
            }
            for (k, &s) in place.calls.iter().enumerate() {
                for sub in &slots[s].subscriptions {
                    subscribers.push((
                        sub.event,
                        Subscriber {
                            place: p,
                            target: Target::Call(k),
                            bit: None,
                            scope: sub.scope,
                        },
                    ));
                }
            }
        }
        subscribers.sort_by_key(|&(e, _)| e);
        let outbox = Outbox::new();
        // Invariant: the root's components have no needs at a parent
        // (`BuildAt` for `End`), so building it lends nothing.
        let root =
            <PlacePositionsAt<Q, I0> as PlaceOps<Q, Place<End>>>::build(None, &(), &args, &outbox)
                .expect("building the root lends nothing");
        let now = clock.now();
        let mut driver = Self {
            root: Some(root),
            args,
            calls: Default::default(),
            nodes: Vec::new(),
            places,
            slots,
            subscribers,
            shared: Arc::new(Shared::default()),
            outer: None,
            epoch: 0,
            clock,
            sleep: None,
            timers: BTreeSet::new(),
            deadlines: BTreeMap::new(),
            outbox,
            pending: Vec::new(),
            serial: 0,
            now,
            remains: None,
            ended: false,
            running: false,
            on_drop: Self::end_dropped,
        };
        let id = driver.register(Address::here(), 0, None);
        let mut todo = Vec::new();
        driver.started(id, &mut todo);
        driver.settle(todo);
        Ok(driver)
    }

    /// The root, while it has not ended; for reading what the calls and
    /// hooks keep.
    pub fn root(&self) -> Option<&RootNode<Q>> {
        self.root.as_ref()
    }
    /// Whether the root ended.
    pub fn ended(&self) -> bool {
        self.ended
    }
    /// What the parts keep of the root's shares after it ended, once it
    /// did.
    pub fn remains(&self) -> Option<&Q::Remains> {
        self.remains.as_ref()
    }

    /// Add a node of the kind `CK` under the node at `parent`, under
    /// `key`, as a request's edit handle does: it is built from the view
    /// of its parent, its calls start woken, its deadlines are read and
    /// [`NodeStarted`](crate::event::NodeStarted) is published at it.
    /// Returns its path.
    #[allow(clippy::type_complexity)]
    pub fn add<CK, X, I>(
        &mut self,
        parent: X,
        key: KeyAt<ParentChildren<RootOf<Q>, X>, I>,
    ) -> Result<<X as Append<I, HandleAt<ParentChildren<RootOf<Q>, X>, I>>>::Out, AddError>
    where
        CK: KindFor<PartsOf<Q>>,
        X: NodePath<RootOf<Q>, PartsOf<Q>> + Append<I, HandleAt<ParentChildren<RootOf<Q>, X>, I>>,
        ParentChildren<RootOf<Q>, X>: ChildAt<I, PartsOf<Q>> + ChildOfKind<CK, I>,
        <X as Append<I, HandleAt<ParentChildren<RootOf<Q>, X>, I>>>::Out: FromAddress<RootOf<Q>>,
        I: Number,
    {
        if self.ended {
            return Err(AddError::Ended);
        }
        let Some(parent_id) = self.find(&parent.address()) else {
            return Err(AddError::NoParent);
        };
        self.now = self.clock.now();
        let mut todo = Vec::new();
        let added = self.add_node(parent_id, I::VALUE, key, &mut todo);
        self.settle(todo);
        let id = added.ok_or(AddError::Occupied)?;
        let addr = self.live(id).map(|n| n.addr).ok_or(AddError::Ended)?;
        // Invariant: the node was added under `parent` at the position `I`,
        // so its address is one of the path type `parent` and `I` give.
        Ok(FromAddress::from_address(&addr).expect("the address of a node added at the path"))
    }

    /// Remove the node at `at`, for `reason`, with every node below it
    /// (each for its kind's default reason), as a request's edit handle
    /// does. The root is ended with [`end`](Driver::end). Returns whether
    /// a node was there and was removed: a node others link to is removed
    /// after the last of them, not on its own.
    pub fn remove<X>(&mut self, at: X, reason: EndReasonOf<TargetOf<Q, X>>) -> bool
    where
        X: PathFrom<RootOf<Q>>,
    {
        let Some(id) = self.find(&at.address()) else {
            return false;
        };
        if id == self.root_id() || self.linked_to(id) {
            return false;
        }
        self.now = self.clock.now();
        let mut todo = Vec::new();
        self.end_with(id, &reason, &mut todo);
        self.settle(todo);
        true
    }

    /// End the root, for `reason`, with every node below it: the nodes
    /// below end first; the root's components write what their parts keep
    /// ([`remains`](Driver::remains)) and end. The driver's future is
    /// ready afterwards.
    pub fn end(&mut self, reason: EndReasonOf<RootOf<Q>>) {
        if self.ended {
            return;
        }
        self.now = self.clock.now();
        let id = self.root_id();
        let mut todo = Vec::new();
        self.end_with(id, &reason, &mut todo);
    }

    // ---- nodes ----

    fn root_id(&self) -> usize {
        // Invariant: the root is registered first and keeps its number
        // until it ends.
        0
    }
    fn live(&self, id: usize) -> Option<&NodeEntry<AddressOf<Q>>> {
        self.nodes.get(id).and_then(NodeSlot::live)
    }
    fn find(&self, addr: &AddressOf<Q>) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.live().is_some_and(|n| n.addr == *addr))
    }
    fn live_ids(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.nodes.len()).filter(|&id| self.live(id).is_some())
    }
    fn depth(&self, id: usize) -> usize {
        self.live(id).map_or(0, |n| self.places[n.place].info.depth)
    }
    /// The node at `depth` on the way up from `id`, `id` included.
    fn ancestor_at(&self, id: usize, depth: usize) -> Option<usize> {
        let mut at = id;
        loop {
            let n = self.live(at)?;
            let d = self.places[n.place].info.depth;
            if d == depth {
                return Some(at);
            }
            if d < depth {
                return None;
            }
            at = n.parent?;
        }
    }
    /// Whether `above` is strictly above `id`.
    fn is_above(&self, above: usize, id: usize) -> bool {
        let mut at = self.live(id).and_then(|n| n.parent);
        while let Some(a) = at {
            if a == above {
                return true;
            }
            at = self.live(a).and_then(|n| n.parent);
        }
        false
    }
    /// Whether nodes link to the node `id`.
    fn linked_to(&self, id: usize) -> bool {
        let _ = id;
        false
    }
    fn register(&mut self, addr: AddressOf<Q>, place: usize, parent: Option<usize>) -> usize {
        let id = self
            .nodes
            .iter()
            .position(|n| match n {
                NodeSlot::Free => true,
                NodeSlot::Live(_) => false,
                NodeSlot::Retired(h) => NodeHead::reusable(h),
            })
            .unwrap_or_else(|| {
                self.nodes.push(NodeSlot::Free);
                self.nodes.len() - 1
            });
        let positions = self.places[place].positions();
        self.serial += 1;
        self.nodes[id] = NodeSlot::Live(NodeEntry {
            head: Arc::new(NodeHead::new(id)),
            serial: self.serial,
            place,
            addr,
            parent,
            children: Vec::new(),
            wakers: vec![None; positions],
        });
        if let Some(parent) = parent
            && let NodeSlot::Live(p) = &mut self.nodes[parent]
        {
            p.children.push(id);
        }
        id
    }
    /// The waker of the position `bit` of the node `id`, made with the
    /// current outer waker.
    fn waker(&mut self, id: usize, bit: usize) -> Waker {
        let epoch = self.epoch;
        let outer = self.outer.clone().unwrap_or_else(|| Waker::noop().clone());
        let shared = self.shared.clone();
        let NodeSlot::Live(n) = &mut self.nodes[id] else {
            return Waker::noop().clone();
        };
        match &n.wakers[bit] {
            Some((e, w)) if *e == epoch => w.clone(),
            _ => {
                let w = Waker::from(Arc::new(PositionWaker {
                    shared,
                    head: n.head.clone(),
                    bit: 1 << bit,
                    outer,
                }));
                n.wakers[bit] = Some((epoch, w.clone()));
                w
            }
        }
    }
    fn mark(&self, id: usize, bits: u64) {
        if let Some(n) = self.live(id) {
            self.shared.mark(&n.head, bits, self.outer.as_ref());
        }
    }
    /// Take the nodes marked, with their marks, in the order of their
    /// first mark.
    fn take_ready(&mut self) -> Vec<(usize, u64)> {
        let ids = self
            .shared
            .take(|id| self.nodes.get(id).and_then(NodeSlot::head));
        let mut out = Vec::new();
        for id in ids {
            let Some(head) = self.nodes.get(id).and_then(NodeSlot::head) else {
                continue;
            };
            let bits = head.marks.swap(0, SeqCst);
            if self.live(id).is_some() && bits != 0 {
                out.push((id, bits));
            }
        }
        out
    }

    /// A new node: its calls start, its calls and its actions before the
    /// calls are woken, its deadlines are read, and that it started is
    /// published.
    fn started(&mut self, id: usize, todo: &mut Vec<Hit>) {
        let p = self.nodes[id].live().map(|n| n.place).unwrap_or(0);
        let addr = self.nodes[id]
            .live()
            .map(|n| n.addr)
            .unwrap_or_else(Address::here);
        for k in 0..self.places[p].calls.len() {
            let s = self.places[p].calls[k];
            let _ = <Cursor<I0> as CallWalk<Q>>::visit(
                s,
                &mut self.calls,
                Start::<Q> {
                    node: id,
                    at: &addr,
                },
            );
        }
        self.woken(id, todo);
    }
    /// A node started or moved: its calls and its actions before the calls
    /// are woken, its deadlines are read, and that it started is
    /// published.
    fn woken(&mut self, id: usize, todo: &mut Vec<Hit>) {
        let p = self.nodes[id].live().map(|n| n.place).unwrap_or(0);
        let woken = self.places[p].calls_mask() | self.places[p].before_mask();
        self.mark(id, woken);
        for c in 0..self.places[p].info.comps.len() {
            if self.places[p].info.comps[c].timed {
                self.read_deadline(id, c);
            }
        }
        let started = self.places[p].info.started;
        self.deliver(id, started, None, false, todo);
    }

    /// Build a node at the position `child` among the children of the
    /// node `parent`, under `key`; `None` when the key is taken.
    fn add_node<KK: Clone + 'static>(
        &mut self,
        parent: usize,
        child: usize,
        key: KK,
        todo: &mut Vec<Hit>,
    ) -> Option<usize> {
        let n = self.live(parent)?;
        let (pp, parent_addr) = (n.place, n.addr);
        let p = self.places[pp].children.get(child).copied().flatten()?;
        let root = self.root.as_mut()?;
        let addr = <Cursor<I0> as PlaceWalk<Q>>::visit(
            p,
            AddAt::<Q, KK> {
                root,
                parent: &parent_addr,
                key,
                args: &self.args,
                outbox: &self.outbox,
            },
        )
        .flatten()?;
        // what the builds published, at the parent and above
        self.effects(parent, &[], None, todo);
        let id = self.register(addr, p, Some(parent));
        self.started(id, todo);
        Some(id)
    }
    /// The node at the position `child` among the children of the node
    /// `parent`, kept under `key`.
    fn find_child<KK: 'static>(&self, parent: usize, child: usize, key: &KK) -> Option<usize> {
        let n = self.live(parent)?;
        let p = self.places[n.place]
            .children
            .get(child)
            .copied()
            .flatten()?;
        let addr = <Cursor<I0> as PlaceWalk<Q>>::visit(
            p,
            FindChild::<Q, KK> {
                root: self.root.as_ref()?,
                parent: &n.addr,
                key,
            },
        )
        .flatten()?;
        self.find(&addr)
    }
    /// End the node `id`, with the nodes below it, for `reason`, of the
    /// type of its kind's reason; the root ends the driver.
    fn end_with<RR: 'static>(&mut self, id: usize, reason: &RR, todo: &mut Vec<Hit>) {
        let Some(p) = self.live(id).map(|n| n.place) else {
            return;
        };
        self.remove_subtree(
            id,
            |root, at, outbox| {
                let _ = <Cursor<I0> as PlaceWalk<Q>>::visit(
                    p,
                    EndWith::<Q, RR> {
                        root,
                        at,
                        reason,
                        outbox,
                    },
                );
            },
            todo,
        );
    }
    /// Every node in the subtree of `id`, `id` first, each node before the
    /// nodes below it.
    fn subtree(&self, id: usize, out: &mut Vec<usize>) {
        if let Some(n) = self.live(id) {
            out.push(id);
            for &c in &n.children {
                self.subtree(c, out);
            }
        }
    }

    /// Every node below and at `id`, in the order they end: a node after
    /// the nodes below it, a child of a kind others link to after its
    /// siblings' subtrees.
    fn removal_order(&self, id: usize, out: &mut Vec<usize>) {
        if let Some(n) = self.live(id) {
            let (targets, others): (Vec<usize>, Vec<usize>) = n.children.iter().partition(|&&c| {
                self.live(c)
                    .is_some_and(|c| self.places[c.place].link_target)
            });
            for c in others.into_iter().chain(targets) {
                self.removal_order(c, out);
            }
            out.push(id);
        }
    }

    /// End the node `id` with the nodes below it; `own_end` runs the end
    /// hooks of `id` itself. What it reaches goes into `todo`; ending the
    /// root ends the driver.
    fn remove_subtree(
        &mut self,
        id: usize,
        own_end: impl FnOnce(&mut RootNode<Q>, &AddressOf<Q>, &Outbox),
        todo: &mut Vec<Hit>,
    ) {
        let mut order = Vec::new();
        self.removal_order(id, &mut order);
        let is_root = id == self.root_id();
        let mut own_end = Some(own_end);
        for n in order {
            let Some(entry) = self.live(n) else { continue };
            let (p, addr) = (entry.place, entry.addr);
            let ending = self.places[p].info.ending;
            self.deliver(n, ending, None, true, todo);
            for c in 0..self.places[p].info.comps.len() {
                self.set_deadline(n, c, None);
            }
            for k in 0..self.places[p].calls.len() {
                let s = self.places[p].calls[k];
                let _ = <Cursor<I0> as CallWalk<Q>>::visit(s, &mut self.calls, Stop { node: n });
            }
            let Some(root) = self.root.as_mut() else {
                return;
            };
            if n == id {
                if is_root {
                    let mut remains = Default::default();
                    <PlacePositionsAt<Q, I0> as PlaceOps<Q, Place<End>>>::remains(
                        root,
                        &addr,
                        &mut remains,
                        &self.outbox,
                    );
                    self.remains = Some(remains);
                }
                if let Some(end) = own_end.take() {
                    end(root, &addr, &self.outbox);
                }
            } else {
                let _ = <Cursor<I0> as PlaceWalk<Q>>::visit(
                    p,
                    EndDefault::<Q> {
                        root,
                        at: &addr,
                        outbox: &self.outbox,
                    },
                );
            }
            let writes = self.comp_writes(p);
            self.effects(n, &writes, None, todo);
            if !is_root || n != id {
                let root = self.root.as_mut().expect("the root is there while it ends");
                let _ = <Cursor<I0> as PlaceWalk<Q>>::visit(p, DetachAt::<Q> { root, at: &addr });
            }
            self.retire(n);
        }
        if is_root {
            self.finish();
        }
    }
    fn retire(&mut self, id: usize) {
        let slot = core::mem::replace(&mut self.nodes[id], NodeSlot::Free);
        if let NodeSlot::Live(n) = slot {
            n.head.dead.store(true, SeqCst);
            if let Some(parent) = n.parent
                && let NodeSlot::Live(p) = &mut self.nodes[parent]
            {
                p.children.retain(|&c| c != id);
            }
            self.nodes[id] = NodeSlot::Retired(n.head);
        }
    }
    /// The root ended: the driver does nothing more.
    fn finish(&mut self) {
        self.ended = true;
        self.root = None;
        self.shared.closed.store(true, SeqCst);
        let _ = self.outbox.take();
        self.timers.clear();
        self.deadlines.clear();
        self.sleep = None;
        self.pending.clear();
        if let Some(outer) = &self.outer {
            outer.wake_by_ref();
        }
    }
    fn comp_writes(&self, p: usize) -> Vec<TypeId> {
        let mut out = Vec::new();
        for c in &self.places[p].info.comps {
            out.extend_from_slice(&c.writes);
        }
        out
    }

    /// Move the node `id` under the node `to`, of the place of its parent:
    /// it ends where it was for the nodes above, its subtree's addresses
    /// follow it, and it starts where it goes; its calls go on.
    fn move_node(&mut self, id: usize, to: usize, todo: &mut Vec<Hit>) -> Result<(), MoveError> {
        let n = self.live(id).ok_or(MoveError::NoSource)?;
        let (p, at, from) = (n.place, n.addr, n.parent);
        let dest = self.live(to).ok_or(MoveError::NoDestination)?;
        let to_addr = dest.addr;
        if from == Some(to) {
            return Ok(());
        }
        let root = self.root.as_mut().ok_or(MoveError::NoSource)?;
        let new = <Cursor<I0> as PlaceWalk<Q>>::visit(
            p,
            MoveUnder::<Q> {
                root,
                at: &at,
                to: &to_addr,
            },
        )
        .unwrap_or(Err(MoveError::NoSource))?;
        // it ends where it was: the nodes above it there hear it
        let ending = self.places[p].info.ending;
        self.deliver(id, ending, None, true, todo);
        let levels = self.places[p].info.depth;
        let mut moved = Vec::new();
        self.subtree(id, &mut moved);
        for &m in &moved {
            if let NodeSlot::Live(e) = &mut self.nodes[m] {
                // Invariant: every node of the subtree has the moved node's
                // address as its first levels.
                e.addr = e
                    .addr
                    .splice(&new, levels)
                    .expect("a node below has the address of the node above it first");
            }
        }
        if let Some(from) = from
            && let NodeSlot::Live(f) = &mut self.nodes[from]
        {
            f.children.retain(|&c| c != id);
        }
        if let NodeSlot::Live(t) = &mut self.nodes[to] {
            t.children.push(id);
        }
        if let NodeSlot::Live(e) = &mut self.nodes[id] {
            e.parent = Some(to);
        }
        for &m in &moved[1..] {
            let q = self.live(m).map_or(0, |n| n.place);
            for c in 0..self.places[q].info.comps.len() {
                if self.places[q].info.comps[c].timed {
                    self.read_deadline(m, c);
                }
            }
        }
        self.woken(id, todo);
        Ok(())
    }

    // ---- requests ----

    /// Have the node a request is for handle it; what its edits reach goes
    /// into `todo`.
    fn handle(&mut self, request: RequestsOf<Q>, todo: &mut Vec<Hit>) {
        if self.ended {
            return;
        }
        request.deliver(self);
        todo.append(&mut self.pending);
    }
    /// Run `f` with the hits of the request being handled as its `todo`.
    fn editing<T>(&mut self, f: impl FnOnce(&mut Self, &mut Vec<Hit>) -> T) -> T {
        let mut todo = core::mem::take(&mut self.pending);
        let out = f(self, &mut todo);
        self.pending.append(&mut todo);
        out
    }

    // ---- deadlines ----

    fn set_deadline(&mut self, id: usize, c: usize, at: Option<InstantOf<Q>>) {
        if let Some(old) = self.deadlines.remove(&(id, c)) {
            self.timers.remove(&(old, id, c));
        }
        if let Some(at) = at {
            self.deadlines.insert((id, c), at);
            self.timers.insert((at, id, c));
        }
    }
    fn deadline(&mut self, id: usize, c: usize) -> Option<InstantOf<Q>> {
        let n = self.live(id)?;
        let (p, addr) = (n.place, n.addr);
        let root = self.root.as_mut()?;
        let at = <Cursor<I0> as PlaceWalk<Q>>::visit(
            p,
            Deadline::<Q> {
                root,
                at: &addr,
                c,
                outbox: &self.outbox,
            },
        )
        .flatten();
        // reading a deadline publishes nothing
        let _ = self.outbox.take();
        at
    }
    fn read_deadline(&mut self, id: usize, c: usize) {
        let at = self.deadline(id, c);
        self.set_deadline(id, c, at);
    }

    // ---- events ----

    fn hit(&self, node: usize, target: Target, events: u64) -> Hit {
        Hit {
            node,
            serial: self.live(node).map_or(0, |n| n.serial),
            target,
            events,
        }
    }
    /// After a lend at `id` that took the components `writes` exclusive:
    /// the deadlines of those types to read again, and the events it
    /// published, as hits. `skip` is a call's own position.
    fn effects(
        &mut self,
        id: usize,
        writes: &[TypeId],
        skip: Option<(usize, Target)>,
        out: &mut Vec<Hit>,
    ) {
        for (depth, event) in self.outbox.take() {
            if let Some(at) = self.ancestor_at(id, depth) {
                self.deliver(at, event, skip, false, out);
            }
        }
        if writes.is_empty() {
            return;
        }
        for m in self.live_ids().collect::<Vec<_>>() {
            let p = self.nodes[m].live().map_or(0, |n| n.place);
            for (c, comp) in self.places[p].info.comps.iter().enumerate() {
                if comp.timed && writes.contains(&comp.type_id) {
                    out.push(self.hit(m, Target::Comp(c), 0));
                }
            }
        }
    }
    /// The hits of the event `event` published at `at`: every subscriber
    /// whose scope has it, `skip` aside; `above` for an event that reaches
    /// only the nodes above.
    fn deliver(
        &self,
        at: usize,
        event: TypeId,
        skip: Option<(usize, Target)>,
        above: bool,
        out: &mut Vec<Hit>,
    ) {
        let start = self.subscribers.partition_point(|&(e, _)| e < event);
        let Some(at_place) = self.live(at).map(|n| n.place) else {
            return;
        };
        for &(e, sub) in &self.subscribers[start..] {
            if e != event {
                break;
            }
            for m in self.live_ids() {
                if self.live(m).map(|n| n.place) != Some(sub.place) {
                    continue;
                }
                if skip == Some((m, sub.target)) {
                    continue;
                }
                let heard = match sub.scope {
                    Scope::Around => {
                        if above {
                            self.is_above(m, at)
                        } else {
                            m == at || self.is_above(m, at) || self.is_above(at, m)
                        }
                    }
                    Scope::At { depth, place } => {
                        self.places[at_place].info.pre == place
                            && self.ancestor_at(at, depth).is_some()
                            && self.ancestor_at(at, depth) == self.ancestor_at(m, depth)
                            && (!above || self.is_above(m, at))
                    }
                };
                if heard {
                    out.push(self.hit(m, sub.target, sub.bit.map_or(0, |b| 1 << b)));
                }
            }
        }
    }
    /// Handle the hits, round after round, until a round publishes nothing
    /// heard: a call is woken; a component's handler runs with the events
    /// that reached it, and its deadline is read again. A request a handler
    /// returns is handled at once, and what it reaches goes into the next
    /// round.
    fn settle(&mut self, mut todo: Vec<Hit>) {
        let mut round = 0usize;
        while !todo.is_empty() && !self.ended {
            round += 1;
            let bound = self
                .places
                .iter()
                .map(|p| p.positions() + p.info.comps.len())
                .sum::<usize>()
                * (self.nodes.len() + 1)
                + 2;
            debug_assert!(round <= bound, "events did not settle");
            todo.sort_unstable_by_key(|h| (h.node, h.serial, h.target));
            let mut merged: Vec<Hit> = Vec::with_capacity(todo.len());
            for hit in todo {
                match merged.last_mut() {
                    Some(last)
                        if (last.node, last.serial, last.target)
                            == (hit.node, hit.serial, hit.target) =>
                    {
                        last.events |= hit.events
                    }
                    _ => merged.push(hit),
                }
            }
            let mut next = Vec::new();
            for Hit {
                node,
                serial,
                target,
                events,
            } in merged
            {
                if self.ended {
                    break;
                }
                let Some(n) = self.live(node).filter(|n| n.serial == serial) else {
                    continue;
                };
                let (p, addr) = (n.place, n.addr);
                match target {
                    Target::Call(k) => self.mark(node, 1 << k),
                    Target::Comp(c) => {
                        if events != 0 {
                            let Some(root) = self.root.as_mut() else {
                                break;
                            };
                            let request = <Cursor<I0> as PlaceWalk<Q>>::visit(
                                p,
                                OnEvents::<Q> {
                                    root,
                                    at: &addr,
                                    c,
                                    events: Triggered::new(events, &self.places[p].handled[c]),
                                    outbox: &self.outbox,
                                },
                            )
                            .flatten();
                            let writes = self.places[p].info.comps[c].writes.clone();
                            self.effects(node, &writes, None, &mut next);
                            if let Some(request) = request {
                                self.handle(request, &mut next);
                            }
                        }
                        if self.places[p].info.comps[c].timed
                            && self.live(node).is_some_and(|n| n.serial == serial)
                        {
                            self.read_deadline(node, c);
                        }
                    }
                }
            }
            todo = next;
        }
    }
    /// After a lend at `id`: its effects and the requests it made,
    /// settled.
    fn after_lend(
        &mut self,
        id: usize,
        writes: &[TypeId],
        skip: Option<(usize, Target)>,
        requests: impl IntoIterator<Item = RequestsOf<Q>>,
    ) {
        let mut todo = Vec::new();
        self.effects(id, writes, skip, &mut todo);
        for request in requests {
            self.handle(request, &mut todo);
        }
        self.settle(todo);
    }

    // ---- a poll ----

    fn turn(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.ended {
            return Poll::Ready(());
        }
        if !self.outer.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
            // the wakers made with the old outer waker wake it, not this
            // one: every position is polled once and gets a new waker
            self.outer = Some(cx.waker().clone());
            self.epoch += 1;
            for id in self.live_ids().collect::<Vec<_>>() {
                let p = self.nodes[id].live().map_or(0, |n| n.place);
                let all = self.places[p].calls_mask() | self.places[p].before_mask();
                if let Some(n) = self.live(id) {
                    self.shared.mark(&n.head, all, None);
                }
            }
        }
        self.shared.notified.store(false, SeqCst);
        let now = self.clock.now();
        self.now = now;
        // the actions before the calls, the nodes above first
        let mut ready = self.take_ready();
        let mut before: Vec<(usize, u64)> = ready
            .iter()
            .map(|&(id, bits)| {
                let p = self.nodes[id].live().map_or(0, |n| n.place);
                (id, bits & self.places[p].before_mask())
            })
            .filter(|&(_, b)| b != 0)
            .collect();
        before.sort_by_key(|&(id, _)| self.depth(id));
        for (id, bits) in before {
            let Some(p) = self.live(id).map(|n| n.place) else {
                continue;
            };
            let ncalls = self.places[p].calls.len();
            for j in 0..self.places[p].before.len() {
                if bits & (1 << (ncalls + j)) == 0 || self.ended {
                    continue;
                }
                let Some(addr) = self.live(id).map(|n| n.addr) else {
                    break;
                };
                let c = self.places[p].before[j];
                let waker = self.waker(id, ncalls + j);
                let Some(root) = self.root.as_mut() else {
                    return Poll::Ready(());
                };
                let request = <Cursor<I0> as PlaceWalk<Q>>::visit(
                    p,
                    BeforeCalls::<Q> {
                        root,
                        at: &addr,
                        c,
                        waker: &waker,
                        outbox: &self.outbox,
                    },
                )
                .flatten();
                let writes = self.places[p].info.comps[c].writes.clone();
                self.after_lend(id, &writes, None, request);
            }
        }
        if self.ended {
            return Poll::Ready(());
        }
        // what the actions before the calls woke is polled in this poll;
        // an action before the calls woken since runs in the next poll
        for (id, bits) in self.take_ready() {
            let p = self.nodes[id].live().map_or(0, |n| n.place);
            let again = bits & self.places[p].before_mask();
            self.mark(id, again);
            let calls = bits & self.places[p].calls_mask();
            match ready.iter_mut().find(|(r, _)| *r == id) {
                Some((_, b)) => *b |= calls,
                None => ready.push((id, calls)),
            }
        }
        for (id, bits) in ready {
            let Some(p) = self.live(id).map(|n| n.place) else {
                continue;
            };
            let mut bits = bits & self.places[p].calls_mask();
            while bits != 0 {
                let k = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                // a request may have ended the node or moved it
                let Some(addr) = self.live(id).map(|n| n.addr) else {
                    break;
                };
                let Some(p) = self.live(id).map(|n| n.place) else {
                    break;
                };
                let s = self.places[p].calls[k];
                let waker = self.waker(id, k);
                let mut call_cx = Context::from_waker(&waker);
                let Some(root) = self.root.as_mut() else {
                    return Poll::Ready(());
                };
                let polled = <Cursor<I0> as CallWalk<Q>>::visit(
                    s,
                    &mut self.calls,
                    PollCall::<Q> {
                        root,
                        node: id,
                        at: &addr,
                        outbox: &self.outbox,
                        cx: &mut call_cx,
                    },
                );
                if let Some(Polled::Yielded) = polled {
                    self.mark(id, 1 << k);
                }
                let mut requests = Vec::new();
                <Cursor<I0> as CallWalk<Q>>::take_requests(s, &mut self.calls, &mut requests);
                let writes = self.slots[s].writes.clone();
                self.after_lend(id, &writes, Some((id, Target::Call(k))), requests);
                if self.ended {
                    return Poll::Ready(());
                }
            }
        }
        // the deadlines that came, last
        while let Some(&(at, id, c)) = self.timers.first() {
            if at > now {
                break;
            }
            self.timers.pop_first();
            self.deadlines.remove(&(id, c));
            if self.live(id).is_none() {
                continue;
            }
            match self.deadline(id, c) {
                Some(current) if current <= now => {
                    let Some(n) = self.live(id) else { continue };
                    let (p, addr) = (n.place, n.addr);
                    let Some(root) = self.root.as_mut() else {
                        return Poll::Ready(());
                    };
                    let request = <Cursor<I0> as PlaceWalk<Q>>::visit(
                        p,
                        Expire::<Q> {
                            root,
                            at: &addr,
                            c,
                            now,
                            outbox: &self.outbox,
                        },
                    )
                    .flatten();
                    let mut todo = Vec::new();
                    let writes = self.places[p].info.comps[c].writes.clone();
                    self.effects(id, &writes, None, &mut todo);
                    if self.live(id).is_some() {
                        self.read_deadline(id, c);
                        debug_assert!(
                            self.deadlines
                                .get(&(id, c))
                                .is_none_or(|&later| later > now),
                            "after its expiry a deadline is later than the instant it expired at, or gone",
                        );
                    }
                    if let Some(request) = request {
                        self.handle(request, &mut todo);
                    }
                    self.settle(todo);
                    if self.ended {
                        return Poll::Ready(());
                    }
                }
                later => self.set_deadline(id, c, later),
            }
        }
        #[cfg(debug_assertions)]
        self.check_deadlines();
        if let Some(&(earliest, _, _)) = self.timers.first() {
            if self.sleep.as_ref().is_none_or(|(at, _)| earliest < *at) {
                self.sleep = Some((earliest, Box::pin(self.clock.sleep_until(earliest))));
            }
        } else {
            self.sleep = None;
        }
        if let Some((_, sleep)) = self.sleep.as_mut()
            && sleep.as_mut().poll(cx).is_ready()
        {
            self.sleep = None;
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }

    #[cfg(debug_assertions)]
    fn check_deadlines(&mut self) {
        for id in self.live_ids().collect::<Vec<_>>() {
            let p = self.nodes[id].live().map_or(0, |n| n.place);
            for c in 0..self.places[p].info.comps.len() {
                if !self.places[p].info.comps[c].timed {
                    continue;
                }
                let at = self.deadline(id, c);
                debug_assert_eq!(
                    self.deadlines.get(&(id, c)).copied(),
                    at,
                    "a deadline changed and was not read again: a lend wrote what it depends on without taking the component exclusive, or the component does not hear an event that moves it",
                );
            }
        }
    }

    // ---- going away ----

    /// Dropped before the root ended: every component is told, the nodes
    /// above first, and every node ends. After a panic, only the root's
    /// components are told and write their remains, and no node ends.
    fn end_dropped(&mut self) {
        if self.ended {
            return;
        }
        let panicked = self.running;
        let cause = if panicked {
            AbortCause::Panicked
        } else {
            AbortCause::Dropped
        };
        let _ = self.outbox.take();
        let mut told: Vec<usize> = if panicked {
            vec![self.root_id()]
        } else {
            self.live_ids().collect()
        };
        told.sort_by_key(|&id| self.depth(id));
        for id in told {
            let Some(n) = self.live(id) else { continue };
            let (p, addr) = (n.place, n.addr);
            let Some(root) = self.root.as_mut() else {
                return;
            };
            let _ = <Cursor<I0> as PlaceWalk<Q>>::visit(
                p,
                Aborted::<Q> {
                    root,
                    at: &addr,
                    cause,
                    outbox: &self.outbox,
                },
            );
        }
        // nothing written or published is handled
        let _ = self.outbox.take();
        if panicked {
            if let Some(root) = self.root.as_mut() {
                let mut remains = Default::default();
                <PlacePositionsAt<Q, I0> as PlaceOps<Q, Place<End>>>::remains(
                    root,
                    &Address::here(),
                    &mut remains,
                    &self.outbox,
                );
                self.remains = Some(remains);
            }
            self.running = false;
            self.finish();
        } else {
            let id = self.root_id();
            self.remove_subtree(
                id,
                |root, at, outbox| {
                    <PlacePositionsAt<Q, I0> as PlaceOps<Q, Place<End>>>::end_default(
                        root, at, outbox,
                    )
                },
                &mut Vec::new(),
            );
        }
    }
}

// ---------------------------------------------------------------- the edit handle's operations

impl<Q: DriverComposition> EditOps for Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
    fn add_child<KK: Clone + 'static>(&mut self, node: usize, child: usize, key: KK) -> bool {
        self.editing(|d, todo| d.add_node(node, child, key, todo).is_some())
    }
    fn end_child<KK: 'static, RR: 'static>(
        &mut self,
        node: usize,
        child: usize,
        key: &KK,
        reason: RR,
    ) -> bool {
        let Some(id) = self.find_child(node, child, key) else {
            return false;
        };
        if self.linked_to(id) {
            return false;
        }
        self.editing(|d, todo| d.end_with(id, &reason, todo));
        true
    }
    fn replace_child<KK: Clone + 'static, RR: 'static>(
        &mut self,
        node: usize,
        child: usize,
        key: KK,
        reason: RR,
    ) -> bool {
        let Some(id) = self.find_child(node, child, &key) else {
            return false;
        };
        self.editing(|d, todo| {
            d.end_with(id, &reason, todo);
            d.add_node(node, child, key, todo).is_some()
        })
    }
    fn move_grandchild<KF: 'static, KG: 'static>(
        &mut self,
        node: usize,
        child: usize,
        from: &KF,
        grandchild: usize,
        key: &KG,
        to: &KF,
    ) -> Result<(), MoveError> {
        let from = self
            .find_child(node, child, from)
            .ok_or(MoveError::NoSource)?;
        let moved = self
            .find_child(from, grandchild, key)
            .ok_or(MoveError::NoSource)?;
        let to = self
            .find_child(node, child, to)
            .ok_or(MoveError::NoDestination)?;
        self.editing(|d, todo| d.move_node(moved, to, todo))
    }
    fn end_node<RR: 'static>(&mut self, node: usize, reason: RR) {
        if self.linked_to(node) {
            return;
        }
        self.editing(|d, todo| d.end_with(node, &reason, todo));
    }
}
impl<Q: DriverComposition> FindNode<RootOf<Q>> for Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
    fn find_node(&self, at: &KeyAddress<RootOf<Q>>) -> Option<(usize, TypeId)> {
        let addr = resolve_node(self.root.as_ref()?, at)?;
        let id = self.find(&addr)?;
        let kind = self.places[self.live(id)?.place].info.kind;
        Some((id, kind))
    }
}

impl<Q: DriverComposition> Future for Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let driver = self.get_mut();
        if driver.running {
            // a panic unwound through the last poll
            driver.end_dropped();
            return Poll::Ready(());
        }
        driver.running = true;
        let turned = driver.turn(cx);
        driver.running = false;
        turned
    }
}

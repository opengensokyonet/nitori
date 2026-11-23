//! The driver: the tree, its calls and its hooks, as one future.
//!
//! A [`Driver`] keeps the root of a [`DriverComposition`]'s tree, every
//! call of every node and the driver's own tables, and runs as a future
//! that ends when the root ends. Internally a node has a number; the
//! places of the tree and the call slots are reached by their numbers
//! (the walk), so no function's type names a whole pack.
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
//! An edit's operations are chosen by type: a request is delivered by its
//! place, and the operations on each child of a place are those of the
//! child's place, found where the composition is proved; a key, a start or
//! a reason is never matched to a place at run time.
//!
//! The links of the structure ([`NodeKind::Linked`]) start and end with the
//! nodes that link: the first node linking to a node it finds absent has
//! it built first, under its own key; the last one to end has it ended
//! after it, told so ([`EndCause::Unlinked`]). A node others link to is not
//! removed on its own.
//!
//! A driver dropped before the root ended tells every component that it
//! was dropped ([`Hooks::aborted`](crate::hooks::Hooks::aborted)), the
//! nodes above first, and ends every node. After a panic through a poll
//! only the root's components are told, their remains are kept, and no
//! node ends.
//!
//!
//! A layer whose deadline is a count at the root, and which hears its own
//! start and reads the instant then:
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
//! #     need::{At, Here, Kind, Mut, Now, Ref, Up},
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
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>, _: &mut ()) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! #[derive(Default)]
//! struct Due {
//!     started: Option<Duration>,
//! }
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Layer> for Due {
//!     type Needs = HList![At<Kind<Root>, Here, Ref<Count>>, At<Up<0>, Here, Now<Duration>>];
//!     type BuildNeeds = HList![At<Up<0>, Here, Ref<Count>>];
//!     type Subscribes = HList![Hear<NodeStarted<Layer>>];
//!     type Timing = HasDeadline;
//!     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Layer>>>(
//!         parents: BuildView<'_, Self, Part1, L, Layer, KP>,
//!         _: &ArgsOf<L>,
//!         _: &mut (),
//!     ) -> Self {
//!         let hlist_pat![count] = parents;
//!         assert_eq!(count.0, 0);
//!         Due::default()
//!     }
//!     fn on_events<KP: HookKeys<Self, Part1, L, Layer>>(
//!         _: nitori_driver::event::Triggered<'_>,
//!         view: HookView<'_, Self, Part1, L, Layer, KP>,
//!     ) -> Option<Self::Returns> {
//!         let hlist_pat![this, _, now] = view;
//!         this.started = Some(now);
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
//! let mut driver = Driver::<Q>::new(Clock, frunk_core::hlist![()], ()).unwrap();
//! let layer = driver.add::<Layer, _, _, _>(End, (), ()).unwrap();
//! assert!(Pin::new(&mut driver).poll(&mut Context::from_waker(Waker::noop())).is_pending());
//! use nitori_driver::structure::NodePath;
//! assert_eq!(layer.node(driver.root().unwrap()).unwrap().data.head.head.started, Some(Duration::ZERO));
//! driver.end(());
//! assert!(Pin::new(&mut driver).poll(&mut Context::from_waker(Waker::noop())).is_ready());
//! ```
//!
//! [`NodeStarted`]: crate::event::NodeStarted
//! [`NodeEnding`]: crate::event::NodeEnding
#[cfg(doc)]
use crate::hooks::EndCause;

pub(crate) mod edit;
pub(crate) mod place;
mod wake;
pub(crate) mod walk;

mod add;
mod construct;
mod deadline;
mod dropped;
mod end;
mod error;
mod events;
mod lookup;
mod moving;
mod turn;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec::Vec,
};
use core::{any::TypeId, pin::Pin, task::Waker};

use frunk_core::indices::Here as I0;
use nitori_clock::MonotonicClock;

pub(crate) use edit::DriverOps;
pub use place::{
    AddressOf, ClockInstantOf, ClockOf, DriverComposition, InstantOf, RequestsOf, RootNode,
    TargetOf,
};
pub(crate) use place::{
    FindPlace, NthPlace, Place, PlaceAt, PlaceDesc, PlaceInfo, PlaceKey, PlaceKeyOf, PlaceNth,
    PlaceOps, PlacePositionsAt, PlacesOf, PlacesUnder,
};
use wake::{NodeHead, Shared};
use walk::{CallWalk, Cursor, PlaceWalk, SlotInfo};

use crate::{
    call::{OutboxOf, PartsOf, RootOf},
    event::Scope,
    hooks::ArgsOf,
    structure::{ChildAddresses, End, NodeKind, RemainsOf},
};

pub use error::{AddError, CompositionError, RemoveError};

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
    /// The place of the root's child its nodes link to.
    link: Option<usize>,
}
impl PlaceRt {
    fn positions(&self) -> usize {
        self.calls.len() + self.before.len()
    }
    fn calls_mask(&self) -> u64 {
        low_bits(self.calls.len())
    }
    fn before_mask(&self) -> u64 {
        // Invariant: the positions are at most 64 (`Driver::new`), so the
        // calls' bits leave room for the shift.
        low_bits(self.before.len())
            .checked_shl(self.calls.len() as u32)
            .unwrap_or(0)
    }
}

/// The lowest `n` bits set, `n` up to 64.
fn low_bits(n: usize) -> u64 {
    match n {
        0 => 0,
        n => u64::MAX >> (64 - n.min(64)),
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
    /// The node it links to.
    link: Option<usize>,
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

/// A node marked since the last poll: its number, the serial it was
/// registered with, and the bits marked.
#[derive(Clone, Copy, Debug)]
struct Ready {
    node: usize,
    serial: u64,
    bits: u64,
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
    outbox: OutboxOf<Q>,
    /// What the edits of the request being handled reach.
    pending: Vec<Hit>,
    /// What a `settle` that reached its bound left, handled in the next
    /// poll.
    deferred: Vec<Hit>,
    /// The timers that came again in one poll, put back after it.
    deferred_timers: Vec<(InstantOf<Q>, usize, usize)>,
    /// The serial of the last node registered.
    serial: u64,
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

impl<Q: DriverComposition> Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
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
}

#[cfg(test)]
mod tests {
    use super::low_bits;

    #[test]
    fn the_low_bits_up_to_a_word() {
        assert_eq!(low_bits(0), 0);
        assert_eq!(low_bits(1), 1);
        assert_eq!(low_bits(63), u64::MAX >> 1);
        assert_eq!(low_bits(64), u64::MAX);
        // 64 calls and none before them: no shift past the word
        assert_eq!(low_bits(0).checked_shl(64).unwrap_or(0), 0);
    }
}

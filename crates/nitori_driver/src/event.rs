//! Events: what happened at a node, published by whoever made it happen
//! and handed to what subscribes to it.
//!
//! An event is only a type ([`Event`]). A call or a hook publishes one at
//! a node on its way up, the node lent included: it writes the need
//! `At<anchor, Here, Publish<E>>` ([`Publish`]), proved with its other
//! needs, and its view has a [`Publisher`] for it. The driver publishes
//! [`NodeStarted`] and [`NodeEnding`] itself.
//!
//! A subscriber says where it hears an event from ([`Subscriptions`]):
//! [`Hear<E>`](Hear) by default, from the node it is at, the nodes above it
//! and the nodes below it, not from the nodes beside it;
//! [`HearAt<E, A, S>`](HearAt) from the nodes the anchor `A` and the steps
//! `S` reach from it, proved where the structure is concrete; and
//! [`ForDeadline<E>`](ForDeadline), heard by default, only to have a
//! component's deadline read again. [`NodeEnding`] reaches only the nodes
//! above the node that ends.
//!
//! After a lend, every event it published reaches its subscribers: a
//! component's handler runs once with every event that reached it in a
//! round ([`Triggered`]), a call is woken. What the handlers publish is
//! handled in the next round, until a round publishes nothing.
use alloc::vec::Vec;
use core::{any::TypeId, cell::RefCell, fmt, marker::PhantomData};

use frunk_core::hlist::{HCons, HNil, Selector};

use crate::{
    lend::{
        Build, Cell, Cx, Flat, FreezeOne, KeyedList, KeyedPositions, KeyedShape, Locate, NeedList,
        Pass1, Pass2, Places,
    },
    need::{Anchor, At, Here, Plain, Steps, StepsPath},
    structure::NodeKind,
};

/// Something that happened at a node; the type is the whole event.
pub trait Event: 'static {}

/// A node of the kind `K` was built: the driver publishes it at the new
/// node, once its calls are started and its deadlines read.
pub struct NodeStarted<K>(PhantomData<fn() -> K>);
impl<K: NodeKind> Event for NodeStarted<K> {}

/// A node of the kind `K` is about to end: the driver publishes it at the
/// node while it is in the tree. It reaches only the nodes above it; its
/// subscribers' handlers run once it is gone.
pub struct NodeEnding<K>(PhantomData<fn() -> K>);
impl<K: NodeKind> Event for NodeEnding<K> {}

// ---------------------------------------------------------------- publishing

/// Take a publisher of the event `E` at the anchor of the need; only with
/// the steps [`Here`].
pub struct Publish<E>(PhantomData<fn() -> E>);

/// Where the events published during a lend are kept until the driver
/// hands them on: for each, how deep its node is (the root at 0) and its
/// type. It also holds the instant `T` the driver read when it was last
/// entered, which the need [`Now`](crate::need::Now) lends; `()` outside a
/// driver.
pub struct Outbox<T = ()> {
    published: RefCell<Vec<(usize, TypeId)>>,
    now: core::cell::Cell<T>,
}
impl Outbox {
    /// An empty outbox, without an instant.
    pub fn new() -> Self {
        Self::at(())
    }
}
impl Default for Outbox {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: Copy> Outbox<T> {
    /// An empty outbox whose instant is `now`.
    pub fn at(now: T) -> Self {
        Self {
            published: RefCell::new(Vec::new()),
            now: core::cell::Cell::new(now),
        }
    }
    /// The instant it holds.
    pub fn now(&self) -> T {
        self.now.get()
    }
    /// Hold `now` from here on.
    pub fn set_now(&self, now: T) {
        self.now.set(now);
    }
}
impl<T> Outbox<T> {
    /// Every event published since the last time, in order, as the depth
    /// of its node and its type.
    pub fn take(&self) -> Vec<(usize, TypeId)> {
        core::mem::take(&mut *self.published.borrow_mut())
    }
}
impl<T> fmt::Debug for Outbox<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outbox")
            .field("published", &self.published.borrow().len())
            .finish()
    }
}

/// The place of the outbox among what is lent; it is not a node's.
pub struct OutboxPlace;
/// The outbox as what is lent holds it, first of every cell.
pub type OutboxCell<'a, T = ()> = Cell<OutboxPlace, &'a Outbox<T>>;
impl<'a, T> FreezeOne for &'a Outbox<T> {
    type Out = &'a Outbox<T>;
    fn freeze(self) -> &'a Outbox<T> {
        self
    }
}

/// Publishes the event `E` at the node of its need's anchor.
pub struct Publisher<'w, E> {
    published: &'w RefCell<Vec<(usize, TypeId)>>,
    depth: usize,
    _event: PhantomData<fn() -> E>,
}
impl<E: Event> Publisher<'_, E> {
    /// `E` happened at the anchor's node.
    pub fn publish(&self) {
        self.published
            .borrow_mut()
            .push((self.depth, TypeId::of::<E>()));
    }
}
impl<E> fmt::Debug for Publisher<'_, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Publisher<{}>({})",
            core::any::type_name::<E>(),
            self.depth
        )
    }
}

/// How deep the node of a place (the positions from it up to the root)
/// is: the root at 0.
pub trait Depth {
    /// The depth.
    const DEPTH: usize;
}
impl Depth for HNil {
    const DEPTH: usize = 0;
}
impl<I, T: Depth> Depth for HCons<I, T> {
    const DEPTH: usize = 1 + T::DEPTH;
}

impl<A, E> Flat for At<A, Here, Publish<E>> {
    type Keyed = HNil;
}
impl<P, L: KeyedShape, A, E: 'static> NeedList<P, L> for At<A, Here, Publish<E>> {
    type View<'w, KP: KeyedList<P, L>> = Publisher<'w, E>;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<'a, Cx, U, A, E> Pass1<'a, Cx, U, ()> for At<A, Here, Publish<E>> {
    type Raw = ();
    type Rem = U;
    fn take(u: U) -> ((), U) {
        ((), u)
    }
}
impl<'a, Ch, P, Fr, A, E, AI, FI, T: 'a> Pass2<'a, Cx<Ch, P>, Fr, (AI, FI, T)>
    for At<A, Here, Publish<E>>
where
    A: Anchor<Ch, P, AI, Pre: Depth>,
    Fr: Selector<OutboxCell<'a, T>, FI>,
{
    type Raw = Publisher<'a, E>;
    fn get(f: &Fr) -> Publisher<'a, E> {
        Publisher {
            published: &f.get().0.published,
            depth: <A::Pre as Depth>::DEPTH,
            _event: PhantomData,
        }
    }
}
impl<Ch, P, U, A, E, AI> Locate<Cx<Ch, P>, U, AI> for At<A, Here, Publish<E>> where
    A: Anchor<Ch, P, AI>
{
}
impl<Cx, U, A, E> Places<Cx, U, ()> for At<A, Here, Publish<E>> {
    type Out = HNil;
}
impl<Cx, A, E> KeyedPositions<Cx, ()> for At<A, Here, Publish<E>> {
    type KP = HNil;
}
impl<'a, P, L, KP, A, E: 'static> Build<'a, P, L, KP, (), Publisher<'a, E>>
    for At<A, Here, Publish<E>>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
{
    fn view(_: (), r2: Publisher<'a, E>) -> Publisher<'a, E> {
        r2
    }
}

// ---------------------------------------------------------------- subscribing

/// Hear the event `E` from the node the subscriber is at, the nodes above
/// it and the nodes below it.
pub struct Hear<E>(PhantomData<fn() -> E>);
/// Hear the event `E` from the nodes the anchor `A` and the steps `S`
/// reach from the subscriber's node; steps to a linked node do not
/// compile:
///
/// ```compile_fail
/// #![feature(impl_trait_in_assoc_type)]
/// # use core::time::Duration;
/// # use frunk_core::{HList, hlist::HNil};
/// # use nitori_driver::{
/// #     call::ShareCalls, composition,
/// #     event::{HearAt, NodeStarted},
/// #     hooks::{ArgsOf, BuildKeyed, BuildView, Hooks, Part, PartList},
/// #     lend::KeyedList,
/// #     need::{Kind, MyLink, ToChild},
/// #     structure::{ByOwnKey, Child, LinkedStart, NodeKind, Share, SlotVec},
/// # };
/// # #[derive(Clone, Default)]
/// # struct Clock;
/// # impl nitori_clock::MonotonicClock for Clock {
/// #     type Instant = Duration;
/// #     type Sleep = core::future::Pending<()>;
/// #     fn now(&self) -> Duration { Duration::ZERO }
/// #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
/// #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
/// #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
/// # }
/// # struct Root;
/// # struct Route;
/// # struct Peer;
/// # impl NodeKind for Root {
/// #     type Children = HList![Child<Route, SlotVec<u32>>, Child<Peer, SlotVec<u32>>];
/// #     type Linked = HNil;
/// # }
/// # impl NodeKind for Route {
/// #     type Children = HNil;
/// #     type Linked = HList![ByOwnKey<Peer>];
/// # }
/// # impl LinkedStart<Peer> for Route {
/// #     fn linked_start(_: &mut ()) {}
/// # }
/// # impl NodeKind for Peer {
/// #     type Children = HNil;
/// #     type Linked = HNil;
/// # }
/// # struct Base;
/// # impl<L> Part<L> for Base { type Args = (); }
/// # impl ShareCalls<Root> for Base { type Calls = HNil; }
/// # impl ShareCalls<Route> for Base { type Calls = HNil; }
/// # impl ShareCalls<Peer> for Base { type Calls = HNil; }
/// # impl Share<Root> for Base { type Data = HNil; }
/// # impl Share<Peer> for Base { type Data = HNil; }
/// /// A route hears its peer start: through a link, which is not heard.
/// struct Hears;
/// impl<L: PartList<L> + 'static> Hooks<Base, L, Route> for Hears {
///     type Subscribes = HList![HearAt<NodeStarted<Peer>, Kind<Root>, ToChild<Peer, MyLink>>];
///     fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Route>>>(
///         _: BuildView<'_, Self, Base, L, Route, KP>,
///         _: &ArgsOf<L>,
///         _: &mut (),
///     ) -> Self {
///         Hears
///     }
/// }
/// impl Share<Route> for Base { type Data = HList![Hears]; }
/// struct Q;
/// composition!(impl driver for Q: Root, HList![Base], Clock);
/// ```
#[allow(clippy::type_complexity)]
pub struct HearAt<E, A, S>(PhantomData<fn() -> (E, A, S)>);
/// Hear the event `E` as [`Hear`] does, only to have the component's
/// deadline read again; its handler does not run for it.
pub struct ForDeadline<E>(PhantomData<fn() -> E>);

/// Which nodes a subscriber hears an event from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Its own node, the nodes above it and the nodes below it.
    Around,
    /// The nodes of the place `place` (the type of its positions up to the
    /// root) whose node at the depth `depth` is the subscriber's own node
    /// at that depth.
    At {
        /// The depth of the anchor.
        depth: usize,
        /// The place of the nodes reached.
        place: TypeId,
    },
}
/// One subscription, as the driver keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subscription {
    /// The event.
    pub event: TypeId,
    /// Where it is heard from.
    pub scope: Scope,
    /// Whether the handler runs for it, or the deadline is only read
    /// again.
    pub handled: bool,
}

/// A list of subscriptions of a subscriber on the chain `Ch` (for the
/// parts `P`); `I` is inferred.
#[diagnostic::on_unimplemented(
    message = "the subscriptions `{Self}` cannot be resolved here",
    label = "a subscription's anchor or steps are not on this structure",
    note = "a subscription is `Hear<E>`, `ForDeadline<E>` or `HearAt<E, Anchor, Steps>`, whose anchor is on the way up and whose steps go down from it"
)]
pub trait Subscriptions<Ch, P, I> {
    /// The subscriptions, in order.
    fn subscriptions(out: &mut Vec<Subscription>);
}
impl<Ch, P> Subscriptions<Ch, P, HNil> for HNil {
    fn subscriptions(_: &mut Vec<Subscription>) {}
}
impl<Ch, P, H: Subscriptions<Ch, P, HI>, T: Subscriptions<Ch, P, TI>, HI, TI>
    Subscriptions<Ch, P, HCons<HI, TI>> for HCons<H, T>
{
    fn subscriptions(out: &mut Vec<Subscription>) {
        H::subscriptions(out);
        T::subscriptions(out);
    }
}
impl<Ch, P, E: Event> Subscriptions<Ch, P, ()> for Hear<E> {
    fn subscriptions(out: &mut Vec<Subscription>) {
        out.push(Subscription {
            event: TypeId::of::<E>(),
            scope: Scope::Around,
            handled: true,
        });
    }
}
impl<Ch, P, E: Event> Subscriptions<Ch, P, ()> for ForDeadline<E> {
    fn subscriptions(out: &mut Vec<Subscription>) {
        out.push(Subscription {
            event: TypeId::of::<E>(),
            scope: Scope::Around,
            handled: false,
        });
    }
}
/// A place reached by steps that do not go through a link.
#[diagnostic::on_unimplemented(
    message = "a subscription's steps go to a linked node",
    label = "`HearAt` with `MyLink`",
    note = "`HearAt` hears the nodes its steps reach down the tree; a linked node (`ToChild<_, MyLink>`) is not one of them"
)]
pub trait WithoutLink {}
impl WithoutLink for HNil {}
impl<T: WithoutLink> WithoutLink for HCons<frunk_core::indices::Here, T> {}
impl<I, T: WithoutLink> WithoutLink for HCons<frunk_core::indices::There<I>, T> {}

impl<Ch, P, E: Event, A, S, AI, SI> Subscriptions<Ch, P, (AI, SI)> for HearAt<E, A, S>
where
    A: Anchor<Ch, P, AI, Pre: Depth>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsPath<A, S, Ch, P, AI, SI>: WithoutLink + 'static,
{
    fn subscriptions(out: &mut Vec<Subscription>) {
        out.push(Subscription {
            event: TypeId::of::<E>(),
            scope: Scope::At {
                depth: <A::Pre as Depth>::DEPTH,
                place: TypeId::of::<StepsPath<A, S, Ch, P, AI, SI>>(),
            },
            handled: true,
        });
    }
}

/// The events of those a component's handler runs for that reached it in
/// one round: one bit for each, in the order of its subscriptions.
#[derive(Clone, Copy)]
pub struct Triggered<'a> {
    bits: u64,
    subscribed: &'a [TypeId],
}
impl<'a> Triggered<'a> {
    /// The events at `bits` of `subscribed`.
    pub fn new(bits: u64, subscribed: &'a [TypeId]) -> Self {
        Self { bits, subscribed }
    }
    /// Whether `E` is among them, through any of the subscriptions to it.
    pub fn contains<E: Event>(&self) -> bool {
        let event = TypeId::of::<E>();
        self.subscribed
            .iter()
            .enumerate()
            .any(|(at, &e)| e == event && self.bits & (1 << at) != 0)
    }
    /// Whether none reached it.
    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }
}
impl fmt::Debug for Triggered<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Triggered({:#b})", self.bits)
    }
}

/// The place of a node, the positions from it up to the root, of the
/// chain `Ch` whose first item is the node.
pub trait OwnPlace {
    /// The positions.
    type Pre: Depth + 'static;
}
impl<K, Pre: Depth + 'static, T> OwnPlace for HCons<crate::need::On<K, Pre>, T> {
    type Pre = Pre;
}

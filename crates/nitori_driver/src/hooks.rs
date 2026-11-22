//! What the driver calls on every component of a node: its hooks.
//!
//! A component is the `Self` of [`Hooks<O, L, K>`](Hooks): `O` is the part
//! whose share it is in, `L` the whole list of parts, `K` the node kind.
//! A component may be generic over the list, and answer differently by
//! which parts are in it. The part `O` comes before `L`, so the crate
//! that defines a part can write the hooks of a generic type of another
//! crate (`impl<X, L, K> Hooks<MyPart, L, K> for TheirType<X>`). A bare
//! type parameter the host chooses cannot be `Self` of a foreign trait
//! (E0210); such a component is kept in [`Held<X>`].
//!
//! A hook reads the tree around it as a call does: the component declares
//! its needs ([`Hooks::Needs`], none by default), and every hook but
//! [`build`](Hooks::build) is an associated function lent the view of the
//! component itself, exclusive, followed by those needs
//! ([`HookView`]). The needs are proved where the driver is composed,
//! with the calls'. [`build`](Hooks::build) is lent the needs
//! [`Hooks::BuildNeeds`] at the parent of the node it builds; a component
//! of the root has none.
//!
//! A component with a deadline also writes [`Timed`], whose instant is the
//! driver clock's; the composition checks it. A component hears events
//! through [`Hooks::Subscribes`] ([`event`](crate::event)).
//!
//! Every part has construction arguments, whose type may depend on the
//! whole list ([`Part<L>`](Part)); the driver puts them in one list
//! ([`ArgsOf<L>`](ArgsOf)) and gives every `build` all of it. A component
//! reads what it needs through a trait of its own crate on that list,
//! answered part by part.
//!
//! ```
//! use frunk_core::{HList, hlist::{HCons, HNil}};
//! use nitori_driver::{
//!     hooks::{ArgsOf, Hooks, Part, PartList},
//!     lend::KeyedList,
//!     structure::NodeKind,
//! };
//!
//! struct Root;
//! impl NodeKind for Root {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! /// A part and its arguments.
//! struct Base;
//! struct BaseArgs {
//!     window: u64,
//! }
//! impl<L> Part<L> for Base {
//!     type Args = BaseArgs;
//! }
//! /// What a set of streams reads of the arguments, part by part.
//! trait StreamArgs {
//!     fn window(&self) -> Option<u64> {
//!         None
//!     }
//! }
//! impl StreamArgs for BaseArgs {
//!     fn window(&self) -> Option<u64> {
//!         Some(self.window)
//!     }
//! }
//! /// The same of the whole list.
//! trait StreamArgsList {
//!     fn window(&self) -> Option<u64>;
//! }
//! impl StreamArgsList for HNil {
//!     fn window(&self) -> Option<u64> {
//!         None
//!     }
//! }
//! impl<A: StreamArgs, T: StreamArgsList> StreamArgsList for HCons<A, T> {
//!     fn window(&self) -> Option<u64> {
//!         self.head.window().or(self.tail.window())
//!     }
//! }
//!
//! /// A component generic over the list of parts.
//! struct Streams {
//!     window: u64,
//! }
//! impl<O, L: PartList<L, Args: StreamArgsList> + 'static, K> Hooks<O, L, K> for Streams {
//!     fn build<KP: KeyedList<L, <HNil as nitori_driver::lend::Flat>::Keyed>>(
//!         _: HNil,
//!         args: &ArgsOf<L>,
//!     ) -> Self {
//!         Streams { window: args.window().unwrap_or(0) }
//!     }
//! }
//! ```
//!
//! A host's bare type is not a component; it goes in [`Held`]:
//!
//! ```compile_fail
//! use frunk_core::hlist::HNil;
//! use nitori_driver::{hooks::{ArgsOf, Hooks, PartList}, lend::{Flat, KeyedList}};
//! trait Estimator {
//!     fn new() -> Self;
//! }
//! struct Base;
//! impl<X: Estimator + 'static, L: PartList<L> + 'static, K> Hooks<Base, L, K> for X {
//!     fn build<KP: KeyedList<L, <HNil as Flat>::Keyed>>(_: HNil, _: &ArgsOf<L>) -> Self {
//!         X::new()
//!     }
//! }
//! ```
//!
//! ```
//! use frunk_core::hlist::HNil;
//! use nitori_driver::{hooks::{ArgsOf, Held, Hooks, PartList}, lend::{Flat, KeyedList}};
//! trait Estimator {
//!     fn new() -> Self;
//! }
//! struct Base;
//! impl<X: Estimator + 'static, L: PartList<L> + 'static, K> Hooks<Base, L, K> for Held<X> {
//!     fn build<KP: KeyedList<L, <HNil as Flat>::Keyed>>(_: HNil, _: &ArgsOf<L>) -> Self {
//!         Held(X::new())
//!     }
//! }
//! ```
use core::task::Waker;

use frunk_core::hlist::{HCons, HNil};

use crate::{
    event::Triggered,
    lend::{Flat, KeyedList, NeedSet, ViewOf},
    need::{At, Here, Mut, Up},
    structure::{NodeKind, Share},
};

/// The construction arguments of the part `Self` in a composition of the
/// parts `L`; their type may depend on the whole list.
pub trait Part<L> {
    /// The arguments.
    type Args;
}
/// The arguments of a list of parts `Self` in a composition of the parts
/// `L`: one per part, in order.
pub trait PartList<L> {
    /// The list of arguments.
    type Args;
}
impl<L> PartList<L> for HNil {
    type Args = HNil;
}
impl<L, P: Part<L>, Ps: PartList<L>> PartList<L> for HCons<P, Ps> {
    type Args = HCons<P::Args, Ps::Args>;
}
/// The arguments of every part of the list `L`.
pub type ArgsOf<L> = <L as PartList<L>>::Args;

/// Why a node's components learn that the driver went away before the
/// node ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortCause {
    /// The driver was dropped; every node is ended after this.
    Dropped,
    /// The driver panicked; only the root's components are told, the nodes
    /// are not ended, and a hook can only mark state and wake.
    Panicked,
}

/// The needs a hook of the component `C` is lent: `C` itself, exclusive,
/// then [`Hooks::Needs`].
pub type OwnNeeds<C, O, L, K> = HCons<At<Up<0>, Here, Mut<C>>, <C as Hooks<O, L, K>>::Needs>;
/// The keyed needs of a hook of `C`.
pub type OwnKeyed<C, O, L, K> = <OwnNeeds<C, O, L, K> as Flat>::Keyed;
/// The view a hook of `C` is lent, its keyed needs at `KP`: `&mut C`, then
/// the view of its needs.
pub type HookView<'w, C, O, L, K, KP> = ViewOf<'w, OwnNeeds<C, O, L, K>, L, KP>;
/// The keyed needs of what `build` of `C` is lent.
pub type BuildKeyed<C, O, L, K> = <<C as Hooks<O, L, K>>::BuildNeeds as Flat>::Keyed;
/// The view `build` of `C` is lent at the parent, its keyed needs at `KP`.
pub type BuildView<'w, C, O, L, K, KP> = ViewOf<'w, <C as Hooks<O, L, K>>::BuildNeeds, L, KP>;
/// Why a node of the kind `K` ends.
pub type EndReasonOf<K> = <K as NodeKind>::EndReason;

/// The positions of the keyed needs of a hook of `C`; a hook reaches them
/// through this bound only.
pub trait HookKeys<C: Hooks<O, L, K>, O, L: PartList<L>, K>:
    KeyedList<L, OwnKeyed<C, O, L, K>>
{
}
impl<C: Hooks<O, L, K>, O, L: PartList<L>, K, KP: KeyedList<L, OwnKeyed<C, O, L, K>>>
    HookKeys<C, O, L, K> for KP
{
}

/// A component without a deadline ([`Hooks::Timing`]).
pub struct NoDeadline;
/// A component with a deadline: it writes [`Timed`].
pub struct HasDeadline;

/// The hooks of a component in the share of the part `O`, in a
/// composition of the parts `L`, in a node of the kind `K`.
///
/// Every hook but [`build`](Hooks::build) does nothing by default. Every
/// hook but `build` is lent [`HookView`]: the component, then its
/// [`Needs`](Hooks::Needs), lent at its node. What a hook returns is a
/// request for the driver ([`Returns`](Hooks::Returns), see
/// [`request`](crate::request)), handled before any call is polled.
pub trait Hooks<O, L: PartList<L>, K>: Sized + 'static {
    /// What the hooks read and write around the node besides the
    /// component: a list of needs, as a call's, anchored at the node.
    type Needs: NeedSet<L> = HNil;
    /// What [`build`](Hooks::build) reads at the parent of the new node: a
    /// list of needs anchored at the parent (`Up<0>` is the parent). The
    /// root's components have none.
    type BuildNeeds: NeedSet<L> = HNil;
    /// The events the component hears: a list of
    /// [`Hear`](crate::event::Hear), [`HearAt`](crate::event::HearAt) and
    /// [`ForDeadline`](crate::event::ForDeadline).
    type Subscribes = HNil;
    /// [`NoDeadline`], or [`HasDeadline`] for a component that writes
    /// [`Timed`].
    type Timing = NoDeadline;
    /// What the hooks return to the driver: `Infallible` (nothing), a
    /// [`NodeRequest`](crate::request::NodeRequest), or a `Coproduct` of
    /// them; the composition proves that it goes into the structure's
    /// requests ([`IntoRequestSum`](crate::request::IntoRequestSum)).
    type Returns: 'static = core::convert::Infallible;
    /// Whether [`before_calls`](Hooks::before_calls) does anything.
    const ACTS_BEFORE_CALLS: bool = false;
    /// The component of a new node, from the view of the parent and the
    /// arguments of every part.
    fn build<KP: KeyedList<L, BuildKeyed<Self, O, L, K>>>(
        parents: BuildView<'_, Self, O, L, K, KP>,
        args: &ArgsOf<L>,
    ) -> Self;
    /// Before any call at the start of a poll: in the first poll after the
    /// node is built and after `waker` was woken; nodes above first. The
    /// waker is the component's own.
    fn before_calls<KP: HookKeys<Self, O, L, K>>(
        view: HookView<'_, Self, O, L, K, KP>,
        waker: &Waker,
    ) -> Option<Self::Returns> {
        let _ = (view, waker);
        None
    }
    /// Events it hears happened: `events` tells which, of those it
    /// subscribes to with its handler. Its deadline, if it has one, is
    /// read again afterwards.
    fn on_events<KP: HookKeys<Self, O, L, K>>(
        events: Triggered<'_>,
        view: HookView<'_, Self, O, L, K, KP>,
    ) -> Option<Self::Returns> {
        let _ = (events, view);
        None
    }
    /// The driver went away before the node ended.
    fn aborted<KP: HookKeys<Self, O, L, K>>(
        view: HookView<'_, Self, O, L, K, KP>,
        cause: AbortCause,
    ) {
        let _ = (view, cause);
    }
    /// The root ends: write what the part keeps of the component after
    /// the end into `out`. Only for the root's components, after the nodes
    /// below it ended and before its own [`end`](Hooks::end).
    fn remains<KP: HookKeys<Self, O, L, K>>(
        view: HookView<'_, Self, O, L, K, KP>,
        out: &mut <O as Share<K>>::Remains,
    ) where
        O: Share<K>,
    {
        let _ = (view, out);
    }
    /// The node ends, for `reason`: the nodes below it ended, and it is
    /// still in the tree; the component is dropped afterwards.
    fn end<KP: HookKeys<Self, O, L, K>>(
        view: HookView<'_, Self, O, L, K, KP>,
        reason: &EndReasonOf<K>,
    ) where
        K: NodeKind,
    {
        let _ = (view, reason);
    }
}

/// A component with a deadline, in the share of the part `O`, in a
/// composition of the parts `L`, in a node of the kind `K`.
///
/// The driver reads the deadline when the node is built, after a lend
/// that took the component exclusive, and after an event the component
/// hears; when it comes, the driver runs [`expire`](Timed::expire).
pub trait Timed<O, L: PartList<L>, K>: Hooks<O, L, K, Timing = HasDeadline> {
    /// The instant of the deadline; the composition checks that it is the
    /// driver clock's.
    type Instant: Ord + Copy;
    /// The earliest deadline, or none.
    fn deadline<KP: HookKeys<Self, O, L, K>>(
        view: &HookView<'_, Self, O, L, K, KP>,
    ) -> Option<Self::Instant>;
    /// The deadline came: do what it is for. The driver calls it only when
    /// the deadline it reads just before is not later than `now`;
    /// afterwards the deadline is later than `now` or gone (debug builds
    /// check it).
    fn expire<KP: HookKeys<Self, O, L, K>>(
        view: HookView<'_, Self, O, L, K, KP>,
        now: Self::Instant,
    ) -> Option<Self::Returns>;
}

/// How the driver reads a component's deadline, by its
/// [`Hooks::Timing`]: a component with [`HasDeadline`] whose instant `T`
/// is the driver clock's.
#[diagnostic::on_unimplemented(
    message = "the component `{C}` has no deadline the driver can read with the instant `{T}`",
    label = "its `Timing`",
    note = "a component whose `Timing` is `HasDeadline` writes `Timed`, with the driver clock's `Instant` as its `Instant`"
)]
pub trait Timing<C: Hooks<O, L, K>, O, L: PartList<L>, K, T> {
    /// Whether the component has a deadline.
    const TIMED: bool;
    /// Its deadline.
    fn deadline<KP: HookKeys<C, O, L, K>>(view: &HookView<'_, C, O, L, K, KP>) -> Option<T>;
    /// Its deadline came.
    fn expire<KP: HookKeys<C, O, L, K>>(
        view: HookView<'_, C, O, L, K, KP>,
        now: T,
    ) -> Option<C::Returns>;
}
impl<C: Hooks<O, L, K>, O, L: PartList<L>, K, T> Timing<C, O, L, K, T> for NoDeadline {
    const TIMED: bool = false;
    fn deadline<KP: HookKeys<C, O, L, K>>(_: &HookView<'_, C, O, L, K, KP>) -> Option<T> {
        None
    }
    fn expire<KP: HookKeys<C, O, L, K>>(
        _: HookView<'_, C, O, L, K, KP>,
        _: T,
    ) -> Option<C::Returns> {
        None
    }
}
impl<C: Timed<O, L, K, Instant = T>, O, L: PartList<L>, K, T> Timing<C, O, L, K, T>
    for HasDeadline
{
    const TIMED: bool = true;
    fn deadline<KP: HookKeys<C, O, L, K>>(view: &HookView<'_, C, O, L, K, KP>) -> Option<T> {
        C::deadline(view)
    }
    fn expire<KP: HookKeys<C, O, L, K>>(
        view: HookView<'_, C, O, L, K, KP>,
        now: T,
    ) -> Option<C::Returns> {
        C::expire(view, now)
    }
}

/// A component whose type is a type parameter the host chooses, kept so
/// that a part can write its hooks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held<X>(pub X);

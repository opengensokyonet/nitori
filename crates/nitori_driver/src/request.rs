//! Requests: what a call or a hook asks a node to change in its own
//! subtree.
//!
//! Each kind of node declares what it can be asked ([`NodeKind::Request`],
//! `Infallible` for nothing) and handles it ([`Request::apply`]) with an
//! edit handle ([`Edit`]) that reaches only the node's own subtree: it
//! adds, ends and replaces the node's children, moves a grandchild from one
//! child to another, and ends the node itself. The handle's operations do
//! what the structure says goes with them: building and ending the nodes
//! others link to, the order of removal, the end hooks with the reason.
//!
//! A request names its node by its key address from the root, whose type
//! says the kind it leads to ([`NodeRequest`], [`KeyAddress`]). What a call yields ([`CallKind::Yield`](crate::call::CallKind::Yield))
//! or a hook returns ([`Hooks::Returns`](crate::hooks::Hooks::Returns)) is
//! one request of a kind of the structure, a sum of some of them, or
//! `Infallible`; it goes into the sum of every kind's requests the
//! structure gives ([`RequestSumOf`]), and the composition proves that it
//! does ([`IntoRequestSum`]). A request for a kind that is not in the
//! structure does not compile.
//!
//! The driver handles a request at once, before it polls any call: it
//! finds the node by the address, and drops the request when no node of
//! the request's place is there. The sum is by place, so the driver knows
//! the place of the alternative it delivers, and the edit handle's
//! operations are those of that place and its children, chosen by type.
//!
//! A component of the root asks the root for its layer:
//!
//! ```
//! #![feature(impl_trait_in_assoc_type)]
//! # use core::{convert::Infallible, pin::Pin, task::{Context, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     call::ShareCalls,
//! #     composition,
//! #     driver::Driver,
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HookKeys, HookView, Hooks, Part, PartList},
//! #     lend::KeyedList,
//! #     request::{Edit, Editor, NodeRequest, Request},
//! #     structure::{AtMostOne, Child, KeyAddress, NodeKind, Share},
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
//! struct Root;
//! struct Layer;
//! struct Other;
//! impl NodeKind for Root {
//!     type Children = HList![Child<Layer, AtMostOne>];
//!     type Linked = HNil;
//!     type Request = AddLayer;
//! }
//! impl NodeKind for Layer {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! impl NodeKind for Other {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! /// The root builds its layer.
//! struct AddLayer;
//! impl Request<Root> for AddLayer {
//!     fn apply<E: Editor<Root>>(self, mut edit: Edit<'_, E, Root>) {
//!         let _ = edit.add::<Layer, _>((), ());
//!     }
//! }
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
//! # impl Share<Layer> for Part1 {
//! #     type Data = HNil;
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Starter];
//! # }
//! /// On the root: asks for its layer before the first calls.
//! struct Starter;
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Starter {
//!     const ACTS_BEFORE_CALLS: bool = true;
//!     type Returns = NodeRequest<Root, Root>;
//! #   fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>, _: &mut ()) -> Self {
//! #       Starter
//! #   }
//!     fn before_calls<KP: HookKeys<Self, Part1, L, Root>>(
//!         _: HookView<'_, Self, Part1, L, Root, KP>,
//!         _: &Waker,
//!     ) -> Option<NodeRequest<Root, Root>> {
//!         Some(NodeRequest::new(KeyAddress::here(), AddLayer))
//!     }
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//!
//! let mut driver = Driver::<Q>::new(Clock, frunk_core::hlist![()], ()).unwrap();
//! let _ = Pin::new(&mut driver).poll(&mut Context::from_waker(Waker::noop()));
//! assert!(driver.root().unwrap().children.head.is_some());
//! ```
//!
//! A request for a kind that is not in the structure does not compile:
//!
//! ```compile_fail
//! #![feature(impl_trait_in_assoc_type)]
//! # use core::{convert::Infallible, pin::Pin, task::{Context, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     call::ShareCalls,
//! #     composition,
//! #     driver::Driver,
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HookKeys, HookView, Hooks, Part, PartList},
//! #     lend::KeyedList,
//! #     request::{Edit, Editor, NodeRequest, Request},
//! #     structure::{AtMostOne, Child, KeyAddress, NodeKind, Share},
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
//! struct Root;
//! struct Layer;
//! struct Other;
//! impl NodeKind for Root {
//!     type Children = HList![Child<Layer, AtMostOne>];
//!     type Linked = HNil;
//!     type Request = AddLayer;
//! }
//! impl NodeKind for Layer {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! impl NodeKind for Other {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! /// The root builds its layer.
//! struct AddLayer;
//! impl Request<Root> for AddLayer {
//!     fn apply<E: Editor<Root>>(self, mut edit: Edit<'_, E, Root>) {
//!         let _ = edit.add::<Layer, _>((), ());
//!     }
//! }
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
//! # impl Share<Layer> for Part1 {
//! #     type Data = HNil;
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Starter];
//! # }
//! /// On the root: asks for its layer before the first calls.
//! struct Starter;
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Starter {
//!     const ACTS_BEFORE_CALLS: bool = true;
//!     type Returns = NodeRequest<Root, Other>;
//! #   fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>, _: &mut ()) -> Self {
//! #       Starter
//! #   }
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
//!
//! A request names its node by an address of the node's kind; an address
//! that leads to another kind does not compile:
//!
//! ```compile_fail
//! use frunk_core::{HList, hlist::HNil};
//! use nitori_driver::{
//!     request::{Edit, Editor, NodeRequest, Request},
//!     structure::{AtMostOne, Child, KeyAddress, NodeKind},
//! };
//! struct Root;
//! struct Layer;
//! impl NodeKind for Root {
//!     type Children = HList![Child<Layer, AtMostOne>];
//!     type Linked = HNil;
//!     type Request = AddLayer;
//! }
//! impl NodeKind for Layer {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! struct AddLayer;
//! impl Request<Root> for AddLayer {
//!     fn apply<E: Editor<Root>>(self, mut edit: Edit<'_, E, Root>) {
//!         let _ = edit.add::<Layer, _>((), ());
//!     }
//! }
//! // the root's request, at the layer's address
//! let _ = NodeRequest::<Root, Root>::new(KeyAddress::child::<Layer, _>(()), AddLayer);
//! ```
//!
//! A kind handles its requests on its own subtree; another subtree is not
//! reachable through the handle:
//!
//! ```compile_fail
//! use frunk_core::{HList, hlist::HNil};
//! use nitori_driver::{
//!     request::{Edit, Editor, Request},
//!     structure::{AtMostOne, Child, NodeKind},
//! };
//! struct Root;
//! struct Left;
//! struct Right;
//! impl NodeKind for Root {
//!     type Children = HList![Child<Left, AtMostOne>, Child<Right, AtMostOne>];
//!     type Linked = HNil;
//! }
//! impl NodeKind for Left {
//!     type Children = HNil;
//!     type Linked = HNil;
//!     type Request = AddRight;
//! }
//! impl NodeKind for Right {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! /// The left node asks to build its sibling: not in its subtree.
//! struct AddRight;
//! impl Request<Left> for AddRight {
//!     fn apply<E: Editor<Left>>(self, mut edit: Edit<'_, E, Left>) {
//!         let _ = edit.add::<Right, _>((), ());
//!     }
//! }
//! ```
use core::{convert::Infallible, fmt, marker::PhantomData};

use snafu::Snafu;

use frunk_core::{
    coproduct::{CNil, CoprodInjector, Coproduct, CoproductEmbedder},
    hlist::{HCons, HNil},
};

use crate::{
    driver::{AddError, Place},
    structure::{ChildOfKind, KeyAddress, KeyAt, NodeKind, OccupiedError, PathFrom, StorageAt},
};

/// A request a node of the kind `K` handles on its own subtree.
pub trait Request<K: NodeKind>: 'static {
    /// Do what is asked, through `edit`. The driver polls no call before it
    /// returns.
    fn apply<E: Editor<K>>(self, edit: Edit<'_, E, K>);
}
impl<K: NodeKind> Request<K> for Infallible {
    fn apply<E: Editor<K>>(self, _: Edit<'_, E, K>) {
        match self {}
    }
}

pub(crate) mod sealed {
    use frunk_core::indices::{Here, There};

    use super::{EndError, LinkedToError, MoveGrandchildError};
    use crate::{
        driver::{AddError, DriverOps},
        structure::{ChildList, ChildStorage, NodeKind},
    };

    /// What the driver does for an edit handle of a node of the kind `K`,
    /// at its place: the operations on each child are the items of
    /// [`Children`](EditOps::Children), by the child's position.
    pub trait EditOps<K: NodeKind>: DriverOps {
        /// The operations on each child, in the order of `K`'s children.
        type Children: ChildEdits<Self::Q, K::Children>;
        /// End the node, for `reason`; refused when others link to it.
        fn end_node(&mut self, reason: K::EndReason) -> Result<(), LinkedToError>;
    }
    /// The operations on each child of a list `L`, in a composition `Q`:
    /// the first's, then the others'.
    pub trait ChildEdits<Q, L: ChildList> {
        /// The first child's.
        type Head: ChildEdit<Q, L::HeadKind, L::HeadStorage>;
        /// The others'.
        type Tail: ChildEdits<Q, L::Tail>;
    }
    /// The operations on the children of the kind `CK` kept by `S`, of the
    /// node an editor `E` is at.
    pub trait ChildEdit<Q, CK: NodeKind, S: ChildStorage> {
        /// The moves of each grandchild under these children.
        type Grandchildren: GrandchildEdits<Q, S, CK::Children>;
        /// Add one under `key`, from `start`.
        fn add<E: DriverOps<Q = Q>>(
            e: &mut E,
            key: S::Key,
            start: CK::Start,
        ) -> Result<(), AddError>;
        /// End the one under `key`, for `reason`.
        fn end<E: DriverOps<Q = Q>>(
            e: &mut E,
            key: &S::Key,
            reason: CK::EndReason,
        ) -> Result<(), EndError>;
        /// End the one under `key` for `reason` and build a new one from
        /// `start`.
        fn replace<E: DriverOps<Q = Q>>(
            e: &mut E,
            key: S::Key,
            start: CK::Start,
            reason: CK::EndReason,
        ) -> Result<(), EndError>;
    }
    /// The moves of each grandchild of a list `L` under children kept by
    /// `S`.
    pub trait GrandchildEdits<Q, S: ChildStorage, L: ChildList> {
        /// The first grandchild's.
        type Head: GrandchildEdit<Q, S, L::HeadStorage>;
        /// The others'.
        type Tail: GrandchildEdits<Q, S, L::Tail>;
    }
    /// Moving a grandchild kept by `G` under a child kept by `S`.
    pub trait GrandchildEdit<Q, S: ChildStorage, G: ChildStorage> {
        /// Move the grandchild under `key` from the child under `from` to
        /// the one under `to`.
        fn move_to<E: DriverOps<Q = Q>>(
            e: &mut E,
            from: &S::Key,
            key: &G::Key,
            to: &S::Key,
        ) -> Result<(), MoveGrandchildError>;
    }

    /// The item at the position `I` of a list of operations on children.
    pub trait NthChildEdit<Q, L: ChildList, I> {
        /// The item.
        type Out;
    }
    impl<Q, L: ChildList, X: ChildEdits<Q, L>> NthChildEdit<Q, L, Here> for X {
        type Out = X::Head;
    }
    impl<Q, L: ChildList, X: ChildEdits<Q, L>, I> NthChildEdit<Q, L, There<I>> for X
    where
        X::Tail: NthChildEdit<Q, L::Tail, I>,
    {
        type Out = <X::Tail as NthChildEdit<Q, L::Tail, I>>::Out;
    }
    /// The item at the position `I` of a list of moves of grandchildren.
    pub trait NthGrandchildEdit<Q, S: ChildStorage, L: ChildList, I> {
        /// The item.
        type Out;
    }
    impl<Q, S: ChildStorage, L: ChildList, X: GrandchildEdits<Q, S, L>>
        NthGrandchildEdit<Q, S, L, Here> for X
    {
        type Out = X::Head;
    }
    impl<Q, S: ChildStorage, L: ChildList, X: GrandchildEdits<Q, S, L>, I>
        NthGrandchildEdit<Q, S, L, There<I>> for X
    where
        X::Tail: NthGrandchildEdit<Q, S, L::Tail, I>,
    {
        type Out = <X::Tail as NthGrandchildEdit<Q, S, L::Tail, I>>::Out;
    }
}
use sealed::{ChildEdit, EditOps, GrandchildEdit, NthChildEdit, NthGrandchildEdit};

/// What carries out an edit handle's operations on a node of the kind
/// `K`: the driver, at the node's place. Every operation is chosen by the
/// types of the child it is on; nothing is matched at run time.
pub trait Editor<K: NodeKind>: EditOps<K> {}
impl<K: NodeKind, E: EditOps<K>> Editor<K> for E {}

type Qe<E> = <E as crate::driver::DriverOps>::Q;
type ChildOps<E, K> = <E as EditOps<K>>::Children;
type ChildOp<E, K, I> = <ChildOps<E, K> as NthChildEdit<Qe<E>, <K as NodeKind>::Children, I>>::Out;
type GrandchildOp<E, K, I, CK, J> = <<ChildOp<E, K, I> as ChildEdit<
    Qe<E>,
    CK,
    StorageAt<<K as NodeKind>::Children, I>,
>>::Grandchildren as NthGrandchildEdit<
    Qe<E>,
    StorageAt<<K as NodeKind>::Children, I>,
    <CK as NodeKind>::Children,
    J,
>>::Out;

/// The edit handle a request of a node of the kind `K` is handled with: it
/// reaches the node's own subtree only.
///
/// Each operation does what the structure says goes with it: a node built
/// is built from its parent's view and starts its calls; a node that links
/// to another ([`NodeKind::Linked`]) has it built before it when it is not
/// there, and ended after it when no other node links to it any more (a
/// node replaced keeps it for the new one, see [`replace`](Edit::replace)); a
/// node ends after the nodes below it, its end hooks told the reason. The
/// handle is taken by value and ending the node itself consumes it.
pub struct Edit<'e, E, K> {
    editor: &'e mut E,
    _kind: PhantomData<fn() -> K>,
}
impl<'e, E: Editor<K>, K: NodeKind> Edit<'e, E, K> {
    pub(crate) fn new(editor: &'e mut E) -> Self {
        Self {
            editor,
            _kind: PhantomData,
        }
    }
    /// Build a child of the kind `CK` under `key`, starting from `start`;
    /// its position `I` is inferred from its kind. When a child of the
    /// kind is kept under the key, or the node could not be built, nothing
    /// is built.
    pub fn add<CK: NodeKind, I>(
        &mut self,
        key: KeyAt<K::Children, I>,
        start: CK::Start,
    ) -> Result<(), AddError>
    where
        K::Children: ChildOfKind<CK, I>,
        ChildOps<E, K>: NthChildEdit<Qe<E>, K::Children, I>,
        ChildOp<E, K, I>: ChildEdit<Qe<E>, CK, StorageAt<K::Children, I>>,
    {
        <ChildOp<E, K, I> as ChildEdit<Qe<E>, CK, _>>::add(self.editor, key, start)
    }
    /// End the child of the kind `CK` kept under `key`, with the nodes
    /// below it, for `reason`. Refused when none is there, or when nodes
    /// link to it: the structure ends it after the last of them.
    pub fn end<CK: NodeKind, I>(
        &mut self,
        key: &KeyAt<K::Children, I>,
        reason: CK::EndReason,
    ) -> Result<(), EndError>
    where
        K::Children: ChildOfKind<CK, I>,
        ChildOps<E, K>: NthChildEdit<Qe<E>, K::Children, I>,
        ChildOp<E, K, I>: ChildEdit<Qe<E>, CK, StorageAt<K::Children, I>>,
    {
        <ChildOp<E, K, I> as ChildEdit<Qe<E>, CK, _>>::end(self.editor, key, reason)
    }
    /// End the child of the kind `CK` kept under `key` for `reason` and
    /// build a new one in its place from `start`, with nothing in between:
    /// no call is polled and no event is handled before the new one is
    /// built. Refused as [`end`](Edit::end) is, and then none is built.
    /// The node the child links to ([`NodeKind::Linked`]) stays: the old
    /// child is not counted as the last to link to it, and the new one
    /// links to it as it is, with no
    /// [`LinkedStart`](crate::structure::LinkedStart); it does not end or
    /// start again. The nodes below the old child are ended as by
    /// [`end`](Edit::end), the nodes they were the last to link to with
    /// them.
    pub fn replace<CK: NodeKind, I>(
        &mut self,
        key: KeyAt<K::Children, I>,
        start: CK::Start,
        reason: CK::EndReason,
    ) -> Result<(), EndError>
    where
        K::Children: ChildOfKind<CK, I>,
        ChildOps<E, K>: NthChildEdit<Qe<E>, K::Children, I>,
        ChildOp<E, K, I>: ChildEdit<Qe<E>, CK, StorageAt<K::Children, I>>,
    {
        <ChildOp<E, K, I> as ChildEdit<Qe<E>, CK, _>>::replace(self.editor, key, start, reason)
    }
    /// Move the grandchild of the kind `GK` kept under `key`, with the
    /// nodes below it, from the child of the kind `CK` kept under `from`
    /// to the one kept under `to`, keeping its key. It ends where it was
    /// for the nodes above ([`NodeEnding`](crate::event::NodeEnding)) and
    /// starts where it goes ([`NodeStarted`](crate::event::NodeStarted));
    /// its calls go on. Refused, and nothing moves, when a child or the
    /// grandchild is not there or `to` has a child of the kind `GK` under
    /// `key`.
    pub fn move_grandchild<CK: NodeKind, I, GK: NodeKind, J>(
        &mut self,
        from: &KeyAt<K::Children, I>,
        key: &KeyAt<CK::Children, J>,
        to: &KeyAt<K::Children, I>,
    ) -> Result<(), MoveGrandchildError>
    where
        K::Children: ChildOfKind<CK, I>,
        CK::Children: ChildOfKind<GK, J>,
        ChildOps<E, K>: NthChildEdit<Qe<E>, K::Children, I>,
        ChildOp<E, K, I>: ChildEdit<Qe<E>, CK, StorageAt<K::Children, I>>,
        <ChildOp<E, K, I> as ChildEdit<Qe<E>, CK, StorageAt<K::Children, I>>>::Grandchildren:
            NthGrandchildEdit<Qe<E>, StorageAt<K::Children, I>, CK::Children, J>,
        GrandchildOp<E, K, I, CK, J>:
            GrandchildEdit<Qe<E>, StorageAt<K::Children, I>, StorageAt<CK::Children, J>>,
    {
        <GrandchildOp<E, K, I, CK, J> as GrandchildEdit<Qe<E>, _, _>>::move_to(
            self.editor,
            from,
            key,
            to,
        )
    }
    /// End the node itself, with every node below it, for `reason`.
    /// Refused when nodes link to it: it is not ended then, and the
    /// structure ends it after the last of them, told it was unlinked
    /// ([`EndCause::Unlinked`](crate::hooks::EndCause::Unlinked)).
    pub fn end_self(self, reason: K::EndReason) -> Result<(), LinkedToError> {
        self.editor.end_node(reason)
    }
}
impl<E, K> fmt::Debug for Edit<'_, E, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Edit<{}>", core::any::type_name::<K>())
    }
}

/// No child of this kind is kept under the key.
#[derive(Debug, Snafu, PartialEq, Eq, Clone, Copy)]
#[snafu(display("no child of this kind under this key"))]
pub struct NoChildError;
/// Nodes link to the node: the structure ends it after the last of them.
#[derive(Debug, Snafu, PartialEq, Eq, Clone, Copy)]
#[snafu(display("nodes link to it; it ends after the last of them"))]
pub struct LinkedToError;
/// Why an edit handle did not end a child.
#[derive(Debug, Snafu, PartialEq, Eq)]
#[snafu(module)]
pub enum EndError {
    /// No child of the kind is kept under the key.
    #[snafu(display("the child was not ended"))]
    NoChild {
        /// Why.
        source: NoChildError,
    },
    /// Nodes link to it.
    #[snafu(display("the child was not ended"))]
    LinkedTo {
        /// Why.
        source: LinkedToError,
    },
}
/// Why an edit handle did not move a grandchild; nothing moved.
#[derive(Debug, Snafu, PartialEq, Eq)]
#[snafu(module)]
pub enum MoveGrandchildError {
    /// No child is kept under `from`.
    #[snafu(display("no child to move the grandchild from"))]
    NoFrom {
        /// Why.
        source: NoChildError,
    },
    /// The child under `from` keeps no grandchild under `key`.
    #[snafu(display("no grandchild to move"))]
    NoGrandchild {
        /// Why.
        source: NoChildError,
    },
    /// No child is kept under `to`.
    #[snafu(display("no child to move the grandchild to"))]
    NoTo {
        /// Why.
        source: NoChildError,
    },
    /// The child under `to` keeps a grandchild of the kind under `key`.
    #[snafu(display("the grandchild was not moved"))]
    Occupied {
        /// Why.
        source: OccupiedError,
    },
}

/// A request for the node of the kind `K` at the key address `at` from the
/// root, of the kind `R`; the address leads to a node of the kind `K`.
pub struct NodeRequest<R: NodeKind, K: NodeKind> {
    /// The node it is for.
    pub at: KeyAddress<R, K>,
    /// What it asks.
    pub request: K::Request,
}
impl<R: NodeKind, K: NodeKind> NodeRequest<R, K> {
    /// The request `request` for the node at `at`.
    pub fn new(at: KeyAddress<R, K>, request: K::Request) -> Self {
        Self { at, request }
    }
}
impl<R: NodeKind, K: NodeKind> fmt::Debug for NodeRequest<R, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeRequest<{}>", core::any::type_name::<K>())
    }
}

/// The sum of the requests of the nodes of a list of places of a tree
/// whose root is of the kind `R`: one for each place, in order.
pub trait PlaceRequests<R: NodeKind> {
    /// The sum.
    type Sum: 'static;
}
impl<R: NodeKind> PlaceRequests<R> for HNil {
    type Sum = CNil;
}
impl<R: NodeKind, X: PathFrom<R>, T: PlaceRequests<R>> PlaceRequests<R> for HCons<Place<X>, T> {
    type Sum = Coproduct<NodeRequest<R, X::Target>, T::Sum>;
}
/// The sum of every kind's requests of the tree whose root is of the kind
/// `R`, by place in tree order.
pub type RequestSumOf<R> =
    <<R as crate::driver::PlacesUnder<crate::structure::End>>::Out as PlaceRequests<R>>::Sum;

/// What goes into the sum of requests `S`: `Infallible`, one
/// [`NodeRequest`], or a sum of some of them; `I` is inferred where the
/// composition is proved.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not go into the requests of this structure",
    label = "what a call yields or a hook returns",
    note = "a call yields, and a hook returns, `Infallible`, a `NodeRequest<Root, Kind>` for a kind of the structure, or a `Coproduct` of such"
)]
pub trait IntoRequestSum<S, I> {
    /// The request in the sum.
    fn into_sum(self) -> S;
}
/// [`IntoRequestSum`] for `Infallible`.
pub struct NoRequests;
/// [`IntoRequestSum`] for one request, at the position `I` of the sum.
pub struct OneRequest<I>(PhantomData<fn() -> I>);
/// [`IntoRequestSum`] for a sum of requests, at the positions `I`.
pub struct SomeRequests<I>(PhantomData<fn() -> I>);
impl<S> IntoRequestSum<S, NoRequests> for Infallible {
    fn into_sum(self) -> S {
        match self {}
    }
}
impl<S, R: NodeKind, K: NodeKind, I> IntoRequestSum<S, OneRequest<I>> for NodeRequest<R, K>
where
    S: CoprodInjector<NodeRequest<R, K>, I>,
{
    fn into_sum(self) -> S {
        S::inject(self)
    }
}
impl<S, C: CoproductEmbedder<S, I>, I> IntoRequestSum<S, SomeRequests<I>> for C {
    fn into_sum(self) -> S {
        self.embed()
    }
}

//! The edit handle a request is handled with: it reaches the node's own
//! subtree only, and lends the handler's needs at the node.
//!
//! Every operation is chosen by the types of the child it is on: the
//! handle's position ([`EditPos`]) holds, for each child of the kind,
//! what the composition proved of it, so a child's key, start and reason
//! have that child's types, and nothing is matched at run time.
use core::{fmt, marker::PhantomData};

use crate::{
    call::{
        __pack::{Desc, KindDesc, KindOps, PackIndex},
        Composition, ContextOf, PartsOf, PriorityOf, RootOf,
        pack_at::{IxDesc, IxFk, IxOps},
    },
    component::EndCause,
    lend::entry::Lends,
    need::{Need, Pos, View},
    request::HandleRequest,
    structure::{ChildOfKind, KeyAt, MoveError, NoChild, NodeKind, OccupiedError, StorageAt},
};

use super::Core;

mod ops;

#[doc(hidden)]
pub use ops::{ChildOps, ChildOpsList, ChildPos, FkAppend, FkStep, NoChildOps, NthChildOps};

/// Where an edit handle of a node of the kind `K` is: the composition,
/// and what it proved of each child of the kind.
///
/// Code that handles requests takes it as an opaque parameter; the
/// composition gives it.
#[allow(private_interfaces)]
pub trait EditPos<K: NodeKind> {
    /// The composition.
    type Q: Composition;
    /// The node's full key.
    #[doc(hidden)]
    type Fk: Clone + 'static;
    /// What the handle does on each child.
    #[doc(hidden)]
    type Children: ChildOpsList<Self::Q, Self::Fk, K::Children>;
    /// End the node `number` at `fk`, with every node below it, for
    /// `reason`.
    #[doc(hidden)]
    fn end_node(core: &mut Core<Self::Q>, number: u32, fk: &Self::Fk, reason: &K::EndReason);
}

/// Where the edit handle a handler gets is: it also lends the handler's
/// needs at the node.
#[allow(private_interfaces)]
pub trait HandlerPos<K: NodeKind>: EditPos<K> {
    /// The needs lent.
    type Needs: Need;
    /// Their position.
    type NeedsPos: Pos;
    /// Lend the needs at the node at `fk` to `f`.
    #[doc(hidden)]
    fn lend_at<F, O>(core: &mut Core<Self::Q>, fk: &Self::Fk, f: F) -> Option<O>
    where
        F: for<'v> FnOnce(View<'v, Self::Needs, Self::NeedsPos>) -> O;
}

/// The position of the edit handle of a handler at a node of the kind
/// `K` under `Q`, proved as `O`, its needs at `NP`.
#[allow(clippy::type_complexity)]
pub struct HandlerAt<Q, K, O, NP>(PhantomData<fn() -> (Q, K, O, NP)>);
impl<Q, K, O, NP> EditPos<K> for HandlerAt<Q, K, O, NP>
where
    Q: Composition,
    K: NodeKind,
    Desc<K>: KindDesc<Q, Kind = K>,
    O: KindOps<Q, Desc<K>>,
{
    type Q = Q;
    type Fk = <Desc<K> as KindDesc<Q>>::Fk;
    type Children = O::Children;
    #[allow(private_interfaces)]
    fn end_node(core: &mut Core<Q>, number: u32, fk: &Self::Fk, reason: &K::EndReason) {
        core.end_with::<Desc<K>, O>(number, fk, EndCause::Reason(reason));
    }
}
/// The needs of the handler of `K` under `Q`.
type HandlerNeeds<Q, K> = <<K as NodeKind>::Handler as HandleRequest<K, PriorityOf<Q>>>::Needs;
#[allow(private_bounds)]
impl<Q, K, O, NP> HandlerPos<K> for HandlerAt<Q, K, O, NP>
where
    Q: Composition,
    K: NodeKind,
    Desc<K>: KindDesc<Q, Kind = K>,
    O: KindOps<Q, Desc<K>>,
    NP: Pos,
    K::Handler: HandleRequest<K, PriorityOf<Q>>,
    HandlerNeeds<Q, K>: Lends<
            RootOf<Q>,
            PartsOf<Q>,
            K,
            <Desc<K> as KindDesc<Q>>::Il,
            ContextOf<Q>,
            NP,
            Fk = <Desc<K> as KindDesc<Q>>::Fk,
        >,
{
    type Needs = HandlerNeeds<Q, K>;
    type NeedsPos = NP;
    #[allow(private_interfaces)]
    fn lend_at<F, Out>(core: &mut Core<Q>, fk: &Self::Fk, f: F) -> Option<Out>
    where
        F: for<'v> FnOnce(View<'v, Self::Needs, NP>) -> Out,
    {
        let Core { root, cx, .. } = core;
        <HandlerNeeds<Q, K> as Lends<_, _, K, _, _, NP>>::lend_to(root, fk, cx, f)
    }
}

/// The position of the edit handle of a child whose kind is at `IC` of
/// `Q`'s kinds.
pub struct ChildEditAt<Q, IC>(PhantomData<fn() -> (Q, IC)>);
impl<Q, CK, IC> EditPos<CK> for ChildEditAt<Q, IC>
where
    Q: Composition,
    CK: NodeKind,
    IC: PackIndex,
    IxDesc<Q, IC>: KindDesc<Q, Kind = CK>,
{
    type Q = Q;
    type Fk = IxFk<Q, IC>;
    type Children = <IxOps<Q, IC> as KindOps<Q, IxDesc<Q, IC>>>::Children;
    #[allow(private_interfaces)]
    fn end_node(core: &mut Core<Q>, number: u32, fk: &Self::Fk, reason: &CK::EndReason) {
        core.end_with::<IxDesc<Q, IC>, IxOps<Q, IC>>(number, fk, EndCause::Reason(reason));
    }
}

/// The position of the edit handle of the filler after the last child of
/// a list: there is none.
pub struct NoChildEdit<Q>(PhantomData<fn() -> Q>);
impl<Q: Composition> EditPos<NoChild> for NoChildEdit<Q> {
    type Q = Q;
    type Fk = !;
    type Children = frunk_core::hlist::HNil;
    #[allow(private_interfaces)]
    fn end_node(_: &mut Core<Q>, _: u32, fk: &!, _: &()) {
        match *fk {}
    }
}

/// What the handle of `K` at `P` does on its child at `I`.
type ChildOp<K, P, I> = <<P as EditPos<K>>::Children as NthChildOps<
    <P as EditPos<K>>::Q,
    <P as EditPos<K>>::Fk,
    <K as NodeKind>::Children,
    I,
>>::Out;
/// The composition of the handle at `P`.
type Qp<K, P> = <P as EditPos<K>>::Q;
/// The position of the handle of the child at `I` of the handle at `P`.
type ChildEditOf<K, P, I, CK> = <ChildOp<K, P, I> as ChildOps<
    Qp<K, P>,
    <P as EditPos<K>>::Fk,
    CK,
    StorageAt<<K as NodeKind>::Children, I>,
>>::Edit;

/// The edit handle a request for a node of the kind `K` is handled
/// with, at the position `P`: it reaches the node's own subtree only.
///
/// A node added is built from what is above it and starts its calls,
/// each first woken at the priority the one adding it gives; a node ends
/// after the nodes below it, which end for
/// [`EndCause::Above`](crate::component::EndCause::Above), itself for
/// the reason given. A handle lends the handler's needs at its node
/// ([`lend`](Edit::lend)); while they are lent, the handle is borrowed,
/// so the structure cannot be changed, here by adding a child:
///
/// ```compile_fail,E0499
/// # use nitori_driver::{
/// #     __private::calls::{Item, Root},
/// #     driver::{Edit, HandlerPos},
/// # };
/// fn handle<P: HandlerPos<Root>>(mut edit: Edit<'_, Root, P>, p: nitori_driver::call::PriorityOf<P::Q>) {
///     edit.lend(|_view| {
///         let _ = edit.add::<Item, _>(1, (), p);
///     });
/// }
/// ```
pub struct Edit<'e, K: NodeKind, P: EditPos<K>> {
    core: &'e mut Core<P::Q>,
    number: u32,
    fk: P::Fk,
    _kind: PhantomData<fn() -> K>,
}
impl<'e, K: NodeKind, P: EditPos<K>> Edit<'e, K, P> {
    /// The handle at the node `number` at `fk`.
    pub(crate) fn new(core: &'e mut Core<P::Q>, number: u32, fk: P::Fk) -> Self {
        Self {
            core,
            number,
            fk,
            _kind: PhantomData,
        }
    }
    /// Add a child of the kind `CK` under `key`, starting from `start`;
    /// its position `I` is inferred from its kind. Its calls are first
    /// woken at `p`. Refused when a child of the kind is kept under the
    /// key.
    pub fn add<CK: NodeKind, I>(
        &mut self,
        key: KeyAt<K::Children, I>,
        start: CK::Start,
        p: PriorityOf<P::Q>,
    ) -> Result<(), OccupiedError>
    where
        K::Children: ChildOfKind<CK, I>,
        P::Children: NthChildOps<P::Q, P::Fk, K::Children, I>,
        ChildOp<K, P, I>: ChildOps<P::Q, P::Fk, CK, StorageAt<K::Children, I>>,
    {
        let fk = <ChildOp<K, P, I> as ChildOps<_, _, CK, _>>::fk(&self.fk, key);
        <ChildOp<K, P, I> as ChildOps<_, _, CK, _>>::add(self.core, fk, start, p).map(|_| ())
    }
    /// End the child of the kind `CK` kept under `key`, with the nodes
    /// below it, for `reason`. Refused when none is there.
    pub fn end<CK: NodeKind, I>(
        &mut self,
        key: &KeyAt<K::Children, I>,
        reason: CK::EndReason,
    ) -> Result<(), NoChildError>
    where
        K::Children: ChildOfKind<CK, I>,
        P::Children: NthChildOps<P::Q, P::Fk, K::Children, I>,
        ChildOp<K, P, I>: ChildOps<P::Q, P::Fk, CK, StorageAt<K::Children, I>>,
    {
        let fk = <ChildOp<K, P, I> as ChildOps<_, _, CK, _>>::fk(&self.fk, key.clone());
        let number = <ChildOp<K, P, I> as ChildOps<_, _, CK, _>>::number(self.core, &fk)
            .ok_or(NoChildError)?;
        <ChildOp<K, P, I> as ChildOps<_, _, CK, _>>::end(self.core, number, &reason);
        Ok(())
    }
    /// End the child of the kind `CK` kept under `key` for `reason`, as
    /// [`end`](Edit::end) does, and add a new one under the key from
    /// `start`, its calls first woken at `p`, with nothing in between.
    /// Refused, and nothing added, when none is there.
    pub fn replace<CK: NodeKind, I>(
        &mut self,
        key: KeyAt<K::Children, I>,
        start: CK::Start,
        reason: CK::EndReason,
        p: PriorityOf<P::Q>,
    ) -> Result<(), NoChildError>
    where
        K::Children: ChildOfKind<CK, I>,
        P::Children: NthChildOps<P::Q, P::Fk, K::Children, I>,
        ChildOp<K, P, I>: ChildOps<P::Q, P::Fk, CK, StorageAt<K::Children, I>>,
    {
        self.end::<CK, I>(&key, reason)?;
        let fk = <ChildOp<K, P, I> as ChildOps<_, _, CK, _>>::fk(&self.fk, key);
        // The key was freed just above, and nothing adds a node between.
        let added = <ChildOp<K, P, I> as ChildOps<_, _, CK, _>>::add(self.core, fk, start, p);
        debug_assert!(added.is_ok(), "the key of the child replaced is free");
        Ok(())
    }
    /// Move the grandchild of the kind `GK` kept under `key`, with the
    /// nodes below it, from the child of the kind `CK` kept under `from`
    /// to the one kept under `to`, keeping its key. Every node moved is
    /// the same node as before, and its calls go on. Refused, and nothing
    /// moves, when a child or the grandchild is not there, or `to` keeps
    /// a child of the kind `GK` under `key`.
    pub fn move_grandchild<CK: NodeKind, I, GK: NodeKind, J>(
        &mut self,
        from: &KeyAt<K::Children, I>,
        key: &KeyAt<CK::Children, J>,
        to: &KeyAt<K::Children, I>,
    ) -> Result<(), MoveError>
    where
        K::Children: ChildOfKind<CK, I>,
        CK::Children: ChildOfKind<GK, J>,
        P::Children: NthChildOps<P::Q, P::Fk, K::Children, I>,
        ChildOp<K, P, I>: ChildOps<P::Q, P::Fk, CK, StorageAt<K::Children, I>>,
        <ChildEditOf<K, P, I, CK> as EditPos<CK>>::Children: NthChildOps<
                P::Q,
                <ChildOp<K, P, I> as ChildOps<P::Q, P::Fk, CK, StorageAt<K::Children, I>>>::Fk,
                CK::Children,
                J,
            >,
        ChildOp<CK, ChildEditOf<K, P, I, CK>, J>: ChildOps<
                P::Q,
                <ChildOp<K, P, I> as ChildOps<P::Q, P::Fk, CK, StorageAt<K::Children, I>>>::Fk,
                GK,
                StorageAt<CK::Children, J>,
            >,
    {
        type C<K, P, I> = ChildOp<K, P, I>;
        let from_fk = <C<K, P, I> as ChildOps<_, _, CK, _>>::fk(&self.fk, from.clone());
        let to_fk = <C<K, P, I> as ChildOps<_, _, CK, _>>::fk(&self.fk, to.clone());
        <C<K, P, I> as ChildOps<_, _, CK, _>>::number(self.core, &from_fk)
            .ok_or(MoveError::NoSource)?;
        <C<K, P, I> as ChildOps<_, _, CK, _>>::number(self.core, &to_fk)
            .ok_or(MoveError::NoDestination)?;
        type G<K, P, I, CK, J> = ChildOp<CK, ChildEditOf<K, P, I, CK>, J>;
        let g_from = <G<K, P, I, CK, J> as ChildOps<_, _, GK, _>>::fk(&from_fk, key.clone());
        let g_to = <G<K, P, I, CK, J> as ChildOps<_, _, GK, _>>::fk(&to_fk, key.clone());
        <G<K, P, I, CK, J> as ChildOps<_, _, GK, _>>::transplant(self.core, &g_from, &g_to)
    }
    /// The edit handle of the child of the kind `CK` kept under `key`;
    /// `None` when none is there. Its reach is still this node's subtree.
    #[allow(clippy::type_complexity)]
    pub fn child<CK: NodeKind, I>(
        &mut self,
        key: &KeyAt<K::Children, I>,
    ) -> Option<Edit<'_, CK, ChildEditOf<K, P, I, CK>>>
    where
        K::Children: ChildOfKind<CK, I>,
        P::Children: NthChildOps<P::Q, P::Fk, K::Children, I>,
        ChildOp<K, P, I>: ChildOps<P::Q, P::Fk, CK, StorageAt<K::Children, I>>,
    {
        let fk = <ChildOp<K, P, I> as ChildOps<_, _, CK, _>>::fk(&self.fk, key.clone());
        let number = <ChildOp<K, P, I> as ChildOps<_, _, CK, _>>::number(self.core, &fk)?;
        Some(Edit::new(self.core, number, fk))
    }
    /// End the node itself, with every node below it, for `reason`.
    pub fn end_self(self, reason: K::EndReason) {
        P::end_node(self.core, self.number, &self.fk, &reason);
    }
}
impl<K: NodeKind, P: HandlerPos<K>> Edit<'_, K, P> {
    /// Lend the handler's needs at the node to `f`; while they are lent,
    /// the handle cannot change the structure. It lends as often as the
    /// handler likes, between its changes.
    pub fn lend<O>(&mut self, f: impl for<'v> FnOnce(View<'v, P::Needs, P::NeedsPos>) -> O) -> O {
        // Invariant: the handle's node is in the tree while the handle
        // is: only ending the node itself ends it, which takes the handle,
        // and a child's handle borrows its parent's.
        P::lend_at(self.core, &self.fk, f).expect("the node of an edit handle is in the tree")
    }
}
impl<K: NodeKind, P: EditPos<K>> fmt::Debug for Edit<'_, K, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Edit<{}>({})", core::any::type_name::<K>(), self.number)
    }
}

/// No child of this kind is kept under the key.
#[derive(Debug, snafu::Snafu, PartialEq, Eq, Clone, Copy)]
#[snafu(display("no child of this kind under this key"))]
pub struct NoChildError;

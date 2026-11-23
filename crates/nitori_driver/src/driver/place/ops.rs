//! The hooks and the edits of every component of a place, and the pack of
//! the places of a tree.

#[cfg(doc)]
use super::ChildPlace;

use alloc::vec::Vec;
use core::{any::TypeId, task::Waker};

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};

use super::component::P_;
use super::{
    AddressOf, ChildPath, Detach, DriverComposition, EndBy, InstantOf, NoLinkPlace, NoPlace,
    NthPlace, ParentOf, ParentPlace, PartsOps, Place, PlaceDesc, PlaceInfo, PlaceKey, PlaceKeyOf,
    PlacePositions, PlaceShape, PlacesOf, RequestsOf, RootNode, TargetOf,
};

use crate::{
    call::{ChainAt, OutboxOf, RootOf},
    event::{NodeEnding, NodeStarted, OwnPlace, Triggered},
    hooks::{AbortCause, ArgsOf, EndCause, EndReasonOf, StartOf},
    lend::LendError,
    need::ChainOf,
    structure::{
        FromAddress, KindFor, MoveError, Node, NodeKind, ParentPath, PathFrom, RemainsList,
        RemainsOf, Shares,
    },
};

/// The hooks of every component of the place `Pl` of `Q`, reached by the
/// address of a node there; `Self` is the place's positions. A node not
/// there is skipped.
#[allow(clippy::type_complexity, missing_docs)]
pub trait PlaceOps<Q: DriverComposition, Pl: PlaceDesc<RootOf<Q>>> {
    /// The places of its children ([`ChildPlace`]s): the edit handle's
    /// operations on each.
    type Children: crate::request::sealed::ChildEdits<Q, <TargetOf<Q, Pl::At> as NodeKind>::Children>;
    /// The place its nodes link to: building the linked node.
    type Link: crate::driver::edit::LinkAdd<Q, TargetOf<Q, Pl::At>, PlaceKeyOf<Q, Pl::At>>;
    fn info() -> PlaceInfo;
    /// The new node's components, from the parent at `parent` (the root
    /// has none: `root` is `None` then).
    fn build(
        root: Option<&mut RootNode<Q>>,
        parent: &ParentOf<Pl::At>,
        key: &PlaceKeyOf<Q, Pl::At>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, Pl::At>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<Node<TargetOf<Q, Pl::At>, P_<Q>>, LendError>
    where
        TargetOf<Q, Pl::At>: KindFor<P_<Q>>;

    fn deadline(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        o: &OutboxOf<Q>,
    ) -> Option<InstantOf<Q>>;
    fn expire(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>>;
    fn on_events(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>>;
    fn before_calls(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>>;
    fn aborted(root: &mut RootNode<Q>, at: &AddressOf<Q>, cause: AbortCause, o: &OutboxOf<Q>);
    /// Every component's end, for `cause`.
    fn end(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        cause: EndCause<'_, EndReasonOf<TargetOf<Q, Pl::At>>>,
        o: &OutboxOf<Q>,
    );
    /// Every component's end, for a cause that carries no reason.
    fn end_by(root: &mut RootNode<Q>, at: &AddressOf<Q>, by: EndBy, o: &OutboxOf<Q>);
    fn remains(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        out: &mut RemainsOf<P_<Q>, TargetOf<Q, Pl::At>>,
        o: &OutboxOf<Q>,
    ) where
        P_<Q>: RemainsList<TargetOf<Q, Pl::At>>;
    /// Take the node out of its parent's store.
    fn detach(root: &mut RootNode<Q>, at: &AddressOf<Q>) -> bool;
    /// Build a node of the place under the parent at `parent`, under `key`
    /// (of the place's store), and keep it there. The parent is there and
    /// the key is free: the driver checked both.
    fn add(
        root: &mut RootNode<Q>,
        parent: &AddressOf<Q>,
        key: PlaceKeyOf<Q, Pl::At>,
        start: StartOf<TargetOf<Q, Pl::At>>,
        args: &ArgsOf<P_<Q>>,
        o: &OutboxOf<Q>,
    ) -> AddressOf<Q>;
    /// The node of the place kept under `key` under the parent at
    /// `parent`.
    fn find_child(
        root: &RootNode<Q>,
        parent: &AddressOf<Q>,
        key: &PlaceKeyOf<Q, Pl::At>,
    ) -> Option<AddressOf<Q>>;
    /// Move the node at `at` under the node at `to`, of the place of its
    /// parent.
    fn move_under(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        to: &AddressOf<Q>,
    ) -> Result<AddressOf<Q>, MoveError>;
}
impl<Q, X, PL, Ix, Lk> PlaceOps<Q, Place<X>> for PlacePositions<PL, Ix, Lk>
where
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>>
        + FromAddress<RootOf<Q>>
        + ChainOf<RootOf<Q>, HNil>
        + ParentPath
        + ParentPlace
        + Detach<RootOf<Q>, P_<Q>>
        + ChildPath<RootOf<Q>, P_<Q>>
        + PlaceKey<RootOf<Q>>
        + Copy
        + 'static,
    <TargetOf<Q, X> as NodeKind>::Children: crate::structure::ChildStores<P_<Q>>,
    ChainAt<Q, X>: OwnPlace,
    P_<Q>: Shares<TargetOf<Q, X>> + RemainsList<TargetOf<Q, X>>,
    PL: PartsOps<Q, X, P_<Q>>,
    Ix: crate::request::sealed::ChildEdits<Q, <TargetOf<Q, X> as NodeKind>::Children>,
    Lk: crate::driver::edit::LinkAdd<Q, TargetOf<Q, X>, PlaceKeyOf<Q, X>>,
{
    type Children = Ix;
    type Link = Lk;
    fn info() -> PlaceInfo {
        let mut comps = Vec::new();
        PL::info(&mut comps);
        PlaceInfo {
            path: TypeId::of::<X>(),
            parent: X::parent_place(),
            index: X::child_index(),
            kind: TypeId::of::<TargetOf<Q, X>>(),
            links: {
                let mut links = Vec::new();
                <<TargetOf<Q, X> as NodeKind>::Linked as crate::structure::LinkList<
                    TargetOf<Q, X>,
                >>::targets(&mut links);
                links
            },
            pre: TypeId::of::<<ChainAt<Q, X> as OwnPlace>::Pre>(),
            depth: <<ChainAt<Q, X> as OwnPlace>::Pre as crate::event::Depth>::DEPTH,
            started: TypeId::of::<NodeStarted<TargetOf<Q, X>>>(),
            ending: TypeId::of::<NodeEnding<TargetOf<Q, X>>>(),
            comps,
        }
    }
    fn build(
        root: Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &PlaceKeyOf<Q, X>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, X>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<Node<TargetOf<Q, X>, P_<Q>>, LendError> {
        let mut root = root;
        Ok(Node::new(PL::build(
            &mut root, parent, key, args, start, outbox,
        )?))
    }

    fn deadline(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        o: &OutboxOf<Q>,
    ) -> Option<InstantOf<Q>> {
        PL::deadline(root, X::from_address(at)?, c, o)
    }
    fn expire(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        PL::expire(root, X::from_address(at)?, c, now, o)
    }
    fn on_events(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        PL::on_events(root, X::from_address(at)?, c, events, o)
    }
    fn before_calls(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        PL::before_calls(root, X::from_address(at)?, c, waker, o)
    }
    fn aborted(root: &mut RootNode<Q>, at: &AddressOf<Q>, cause: AbortCause, o: &OutboxOf<Q>) {
        if let Some(at) = X::from_address(at) {
            PL::aborted(root, at, cause, o);
        }
    }
    fn end(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        cause: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    ) {
        if let Some(at) = X::from_address(at) {
            PL::end(root, at, cause, o);
        }
    }
    fn end_by(root: &mut RootNode<Q>, at: &AddressOf<Q>, by: EndBy, o: &OutboxOf<Q>) {
        Self::end(root, at, by.cause(), o);
    }
    fn remains(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        out: &mut RemainsOf<P_<Q>, TargetOf<Q, X>>,
        o: &OutboxOf<Q>,
    ) where
        P_<Q>: RemainsList<TargetOf<Q, X>>,
    {
        if let Some(at) = X::from_address(at) {
            PL::remains(root, at, out, o);
        }
    }
    fn detach(root: &mut RootNode<Q>, at: &AddressOf<Q>) -> bool {
        X::from_address(at).is_some_and(|at| at.detach(root))
    }
    fn add(
        root: &mut RootNode<Q>,
        parent: &AddressOf<Q>,
        key: PlaceKeyOf<Q, X>,
        mut start: StartOf<TargetOf<Q, X>>,
        args: &ArgsOf<P_<Q>>,
        o: &OutboxOf<Q>,
    ) -> AddressOf<Q> {
        // Invariant: `parent` is the address of a node of the parent place,
        // which the driver found live; the tree keeps a node at the
        // address of each live node.
        let pp = X::parent_path(parent).expect("the parent's address is of the parent place");
        // Invariant: as above, the parent is in the tree, and a build lends
        // only the parent and the nodes around it, which the tree has.
        let node = Self::build(Some(root), &pp, &key, args, &mut start, o)
            .expect("the parent is lent to the new node's build");
        // Invariant: the parent was reached by the build, and the driver
        // found its key free; a build adds no node.
        X::attach(root, parent, key, node).expect("the new node is kept under a free key")
    }
    fn find_child(
        root: &RootNode<Q>,
        parent: &AddressOf<Q>,
        key: &PlaceKeyOf<Q, X>,
    ) -> Option<AddressOf<Q>> {
        X::find_under(root, parent, key)
    }
    fn move_under(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        to: &AddressOf<Q>,
    ) -> Result<AddressOf<Q>, MoveError> {
        X::move_under(root, at, to)
    }
}

/// The end of a pack of places: never reached.
pub struct NoPlacePos;
const NO_PLACE: &str =
    "the end of a pack of places is never reached: the driver walks the places it has";
impl<Q: DriverComposition> PlaceOps<Q, NoPlace> for NoPlacePos {
    type Children = crate::driver::edit::NoPlaceEdits;
    type Link = NoLinkPlace;
    fn info() -> PlaceInfo {
        unreachable!("{NO_PLACE}")
    }
    fn build(
        _: Option<&mut RootNode<Q>>,
        _: &(),
        _: &(),
        _: &ArgsOf<P_<Q>>,
        _: &mut StartOf<RootOf<Q>>,
        _: &OutboxOf<Q>,
    ) -> Result<RootNode<Q>, LendError> {
        unreachable!("{NO_PLACE}")
    }

    fn deadline(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: usize,
        _: &OutboxOf<Q>,
    ) -> Option<InstantOf<Q>> {
        None
    }
    fn expire(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: usize,
        _: InstantOf<Q>,
        _: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn on_events(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: usize,
        _: Triggered<'_>,
        _: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn before_calls(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: usize,
        _: &Waker,
        _: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn aborted(_: &mut RootNode<Q>, _: &AddressOf<Q>, _: AbortCause, _: &OutboxOf<Q>) {}
    fn end(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: EndCause<'_, EndReasonOf<RootOf<Q>>>,
        _: &OutboxOf<Q>,
    ) {
    }
    fn end_by(_: &mut RootNode<Q>, _: &AddressOf<Q>, _: EndBy, _: &OutboxOf<Q>) {}
    fn remains(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: &mut RemainsOf<P_<Q>, RootOf<Q>>,
        _: &OutboxOf<Q>,
    ) where
        P_<Q>: RemainsList<RootOf<Q>>,
    {
    }
    fn detach(_: &mut RootNode<Q>, _: &AddressOf<Q>) -> bool {
        false
    }
    fn add(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: (),
        _: StartOf<RootOf<Q>>,
        _: &ArgsOf<P_<Q>>,
        _: &OutboxOf<Q>,
    ) -> AddressOf<Q> {
        unreachable!("{NO_PLACE}")
    }
    fn find_child(_: &RootNode<Q>, _: &AddressOf<Q>, _: &()) -> Option<AddressOf<Q>> {
        None
    }
    fn move_under(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: &AddressOf<Q>,
    ) -> Result<AddressOf<Q>, MoveError> {
        Err(MoveError::NoSource)
    }
}

/// The positions of every place of the list `L` under `Q`.
pub trait PlacePack<Q: DriverComposition, L: PlaceShape<RootOf<Q>>> {
    /// The positions of the first place.
    type Head: PlaceOps<Q, L::Head>;
    /// The positions of the others.
    type Tail: PlacePack<Q, L::Tail>;
}
impl<Q: DriverComposition> PlacePack<Q, HNil> for HNil {
    type Head = NoPlacePos;
    type Tail = HNil;
}
impl<Q: DriverComposition, S: PlaceDesc<RootOf<Q>>, L: PlaceShape<RootOf<Q>>, PH, PT>
    PlacePack<Q, HCons<S, L>> for HCons<PH, PT>
where
    PH: PlaceOps<Q, S>,
    PT: PlacePack<Q, L>,
{
    type Head = PH;
    type Tail = PT;
}
/// The positions of the `I`-th place of a pack, through the pack's bounds
/// only.
pub trait PlaceNth<Q: DriverComposition, L: NthPlace<RootOf<Q>, I>, I> {
    /// The positions.
    type Out: PlaceOps<Q, L::Item>;
}
impl<Q: DriverComposition, L: PlaceShape<RootOf<Q>>, P: PlacePack<Q, L>> PlaceNth<Q, L, I0> for P {
    type Out = P::Head;
}
impl<Q: DriverComposition, L: PlaceShape<RootOf<Q>>, P: PlacePack<Q, L>, I> PlaceNth<Q, L, There<I>>
    for P
where
    L::Tail: NthPlace<RootOf<Q>, I>,
    P::Tail: PlaceNth<Q, L::Tail, I>,
{
    type Out = <P::Tail as PlaceNth<Q, L::Tail, I>>::Out;
}
/// The place at the position `I` of `Q`'s tree.
pub type PlaceAt<Q, I> = <PlacesOf<Q> as NthPlace<RootOf<Q>, I>>::Item;
/// The positions of the place at `I`.
pub type PlacePositionsAt<Q, I> =
    <<Q as DriverComposition>::Places as PlaceNth<Q, PlacesOf<Q>, I>>::Out;

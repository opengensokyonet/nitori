//! The edit handle's operations, chosen by type.
//!
//! A request is delivered by its place in the sum of requests
//! ([`Deliver`]); the place's positions hold, for each child, the child's
//! place and positions ([`ChildPlace`]) and, for a kind that links, the
//! linked node's place and positions ([`LinkPlace`]), proved where the
//! composition is. The operations on a child are those of the child's
//! place, so its key, start and reason have the place's types; nothing is
//! matched to a place at run time.
use core::{convert::Infallible, marker::PhantomData};

use frunk_core::{
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};

use super::{
    AddError, Driver,
    place::{
        ChildPlace, DriverComposition, LinkPlace, NoLinkPlace, NthPlace, PlaceAt, PlaceDesc,
        PlaceKey, PlaceKeyOf, PlaceNth, PlaceOps, PlacePositionsAt, PlacesOf, TargetOf,
    },
    walk::{CallWalk, Cursor, Number, PlaceWalk},
};
use crate::{
    call::{PartsOf, RootOf},
    driver::Place,
    hooks::{EndReasonOf, StartOf},
    request::{
        Edit, Editor, EndError, LinkedToError, MoveGrandchildError, NoChildError, NodeRequest,
        Request,
        sealed::{ChildEdit, ChildEdits, EditOps, GrandchildEdit, GrandchildEdits},
    },
    structure::{
        Child, ChildAddresses, ChildList, ChildStorage, End, LinkedStart, NoChild, NodeKind,
        OccupiedError, ParentPath, PathFrom, RemainsList, RemainsOf,
    },
};

/// The typed operations of the driver the edit handle's operations are
/// made of: on the node of the place `Pl` whose positions are `Pos`.
#[allow(missing_docs)]
pub trait DriverOps {
    /// The composition.
    type Q: DriverComposition;
    /// The node the editor is at.
    fn node(&self) -> usize;
    /// The root's number.
    fn root_node(&self) -> usize;
    /// Whether nodes link to the node `id`.
    fn linked_to(&self, id: usize) -> bool;
    /// Build a node of the place `Pl`, at the number `p`, under `parent`.
    fn add_child<Pl: PlaceDesc<RootOf<Self::Q>>, Pos: PlaceOps<Self::Q, Pl>>(
        &mut self,
        p: usize,
        parent: usize,
        key: PlaceKeyOf<Self::Q, Pl::At>,
        start: StartOf<TargetOf<Self::Q, Pl::At>>,
    ) -> Result<usize, AddError>;
    /// The node of the place `Pl` under `parent`, kept under `key`.
    fn find_child<Pl: PlaceDesc<RootOf<Self::Q>>, Pos: PlaceOps<Self::Q, Pl>>(
        &self,
        parent: usize,
        key: &PlaceKeyOf<Self::Q, Pl::At>,
    ) -> Option<usize>;
    /// End the node `id` of the place `Pl`, for `reason`.
    fn end_typed<Pl: PlaceDesc<RootOf<Self::Q>>, Pos: PlaceOps<Self::Q, Pl>>(
        &mut self,
        id: usize,
        reason: EndReasonOf<TargetOf<Self::Q, Pl::At>>,
    );
    /// End the node `id` of the place `Pl` for `reason`, and build one in
    /// its place under `parent`, with nothing in between; returns the new
    /// one.
    #[allow(clippy::too_many_arguments)]
    fn replace_typed<Pl: PlaceDesc<RootOf<Self::Q>>, Pos: PlaceOps<Self::Q, Pl>>(
        &mut self,
        p: usize,
        parent: usize,
        id: usize,
        key: PlaceKeyOf<Self::Q, Pl::At>,
        start: StartOf<TargetOf<Self::Q, Pl::At>>,
        reason: EndReasonOf<TargetOf<Self::Q, Pl::At>>,
    ) -> usize;
    /// Move the node `id` under the node `to`.
    fn move_node(&mut self, id: usize, to: usize) -> Result<(), OccupiedError>;
}

impl<Q: DriverComposition> DriverOps for Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
    type Q = Q;
    fn node(&self) -> usize {
        self.root_id()
    }
    fn root_node(&self) -> usize {
        self.root_id()
    }
    fn linked_to(&self, id: usize) -> bool {
        Driver::linked_to(self, id)
    }
    fn add_child<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(
        &mut self,
        p: usize,
        parent: usize,
        key: PlaceKeyOf<Q, Pl::At>,
        start: StartOf<TargetOf<Q, Pl::At>>,
    ) -> Result<usize, AddError> {
        self.editing(|d, todo| d.add_typed::<Pl, Pos>(p, parent, key, start, todo))
    }
    fn find_child<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(
        &self,
        parent: usize,
        key: &PlaceKeyOf<Q, Pl::At>,
    ) -> Option<usize> {
        self.find_typed::<Pl, Pos>(parent, key)
    }
    fn end_typed<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(
        &mut self,
        id: usize,
        reason: EndReasonOf<TargetOf<Q, Pl::At>>,
    ) {
        self.editing(|d, todo| Driver::end_typed::<Pl, Pos>(d, id, reason, todo));
    }
    fn replace_typed<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(
        &mut self,
        p: usize,
        parent: usize,
        id: usize,
        key: PlaceKeyOf<Q, Pl::At>,
        start: StartOf<TargetOf<Q, Pl::At>>,
        reason: EndReasonOf<TargetOf<Q, Pl::At>>,
    ) -> usize {
        self.editing(|d, todo| {
            Driver::end_typed::<Pl, Pos>(d, id, reason, todo);
            // Invariant: the root is there while a request is handled; the
            // key was freed just above, and nothing was added since; and
            // `parent` did not end with the child. A node that ends with
            // the child is below it, or a linked node no node links to any
            // more, or below one; for `parent` to be among them, a node
            // below a linked node would link, through the nodes below the
            // nodes it links to, back to that linked node. Such a cycle of
            // links does not compile: the positions of each linked node's
            // place would hold their own.
            d.add_typed::<Pl, Pos>(p, parent, key, start, todo)
                .expect("the replacement is built under the parent, under the freed key")
        })
    }
    fn move_node(&mut self, id: usize, to: usize) -> Result<(), OccupiedError> {
        self.editing(|d, todo| Driver::move_node(d, id, to, todo))
    }
}

/// The editor of a node at the place `Pl` whose positions are `Pos`.
pub struct PlaceEdit<'d, Q: DriverComposition, Pl, Pos>
where
    Cursor<I0>: CallWalk<Q>,
{
    driver: &'d mut Driver<Q>,
    node: usize,
    _place: PhantomData<fn() -> (Pl, Pos)>,
}
impl<'d, Q: DriverComposition, Pl, Pos> PlaceEdit<'d, Q, Pl, Pos>
where
    Cursor<I0>: CallWalk<Q>,
{
    fn new(driver: &'d mut Driver<Q>, node: usize) -> Self {
        Self {
            driver,
            node,
            _place: PhantomData,
        }
    }
}
impl<Q: DriverComposition, Pl, Pos> DriverOps for PlaceEdit<'_, Q, Pl, Pos>
where
    Cursor<I0>: CallWalk<Q>,
    Driver<Q>: DriverOps<Q = Q>,
{
    type Q = Q;
    fn node(&self) -> usize {
        self.node
    }
    fn root_node(&self) -> usize {
        self.driver.root_node()
    }
    fn linked_to(&self, id: usize) -> bool {
        DriverOps::linked_to(&*self.driver, id)
    }
    fn add_child<Pc: PlaceDesc<RootOf<Q>>, Ps: PlaceOps<Q, Pc>>(
        &mut self,
        p: usize,
        parent: usize,
        key: PlaceKeyOf<Q, Pc::At>,
        start: StartOf<TargetOf<Q, Pc::At>>,
    ) -> Result<usize, AddError> {
        self.driver.add_child::<Pc, Ps>(p, parent, key, start)
    }
    fn find_child<Pc: PlaceDesc<RootOf<Q>>, Ps: PlaceOps<Q, Pc>>(
        &self,
        parent: usize,
        key: &PlaceKeyOf<Q, Pc::At>,
    ) -> Option<usize> {
        self.driver.find_child::<Pc, Ps>(parent, key)
    }
    fn end_typed<Pc: PlaceDesc<RootOf<Q>>, Ps: PlaceOps<Q, Pc>>(
        &mut self,
        id: usize,
        reason: EndReasonOf<TargetOf<Q, Pc::At>>,
    ) {
        DriverOps::end_typed::<Pc, Ps>(&mut *self.driver, id, reason)
    }
    fn replace_typed<Pc: PlaceDesc<RootOf<Q>>, Ps: PlaceOps<Q, Pc>>(
        &mut self,
        p: usize,
        parent: usize,
        id: usize,
        key: PlaceKeyOf<Q, Pc::At>,
        start: StartOf<TargetOf<Q, Pc::At>>,
        reason: EndReasonOf<TargetOf<Q, Pc::At>>,
    ) -> usize {
        self.driver
            .replace_typed::<Pc, Ps>(p, parent, id, key, start, reason)
    }
    fn move_node(&mut self, id: usize, to: usize) -> Result<(), OccupiedError> {
        DriverOps::move_node(&mut *self.driver, id, to)
    }
}
impl<Q, Pl, Pos> EditOps<TargetOf<Q, Pl::At>> for PlaceEdit<'_, Q, Pl, Pos>
where
    Q: DriverComposition,
    Pl: PlaceDesc<RootOf<Q>>,
    Pos: PlaceOps<Q, Pl>,
    Cursor<I0>: CallWalk<Q>,
    Driver<Q>: DriverOps<Q = Q>,
{
    type Children = Pos::Children;
    fn end_node(&mut self, reason: EndReasonOf<TargetOf<Q, Pl::At>>) -> Result<(), LinkedToError> {
        let node = self.node;
        if DriverOps::linked_to(self, node) {
            return Err(LinkedToError);
        }
        DriverOps::end_typed::<Pl, Pos>(self, node, reason);
        Ok(())
    }
}

// ---------------------------------------------------------------- children

impl<Q> ChildEdits<Q, HNil> for HNil {
    type Head = NoChildPlace;
    type Tail = HNil;
}
impl<Q, CK, S, T, Xc, Pc, N, R> ChildEdits<Q, HCons<Child<CK, S>, T>>
    for HCons<ChildPlace<Xc, Pc, N>, R>
where
    CK: NodeKind,
    S: ChildStorage,
    T: ChildList,
    ChildPlace<Xc, Pc, N>: ChildEdit<Q, CK, S>,
    R: ChildEdits<Q, T>,
{
    type Head = ChildPlace<Xc, Pc, N>;
    type Tail = R;
}

/// The operations on the child of an empty list: there is none.
pub struct NoChildPlace;
impl<Q> ChildEdit<Q, NoChild, NoChild> for NoChildPlace {
    type Grandchildren = NoGrandchildPlace;
    fn add<E: DriverOps<Q = Q>>(_: &mut E, key: Infallible, _: Infallible) -> Result<(), AddError> {
        match key {}
    }
    fn end<E: DriverOps<Q = Q>>(_: &mut E, key: &Infallible, _: ()) -> Result<(), EndError> {
        match *key {}
    }
    fn replace<E: DriverOps<Q = Q>>(
        _: &mut E,
        key: Infallible,
        _: Infallible,
        _: (),
    ) -> Result<(), EndError> {
        match key {}
    }
}
/// The moves of the grandchild of an empty list: there is none.
pub struct NoGrandchildPlace;
impl<Q, S: ChildStorage> GrandchildEdits<Q, S, HNil> for NoGrandchildPlace {
    type Head = NoGrandchildPlace;
    type Tail = NoGrandchildPlace;
}
impl<Q, S: ChildStorage> GrandchildEdit<Q, S, NoChild> for NoGrandchildPlace {
    fn move_to<E: DriverOps<Q = Q>>(
        _: &mut E,
        _: &S::Key,
        key: &Infallible,
        _: &S::Key,
    ) -> Result<(), MoveGrandchildError> {
        match *key {}
    }
}

impl<Q, CK, S, Xc, Pc, N> ChildEdit<Q, CK, S> for ChildPlace<Xc, Pc, N>
where
    Q: DriverComposition,
    CK: NodeKind,
    S: ChildStorage,
    N: Number,
    Xc: ParentPath + PathFrom<RootOf<Q>, Target = CK> + PlaceKey<RootOf<Q>, Key = S::Key> + 'static,
    Pc: PlaceOps<Q, Place<Xc>>,
    GrandchildPlaces<Xc, Pc, Pc::Children>: GrandchildEdits<Q, S, CK::Children>,
{
    type Grandchildren = GrandchildPlaces<Xc, Pc, Pc::Children>;
    fn add<E: DriverOps<Q = Q>>(e: &mut E, key: S::Key, start: CK::Start) -> Result<(), AddError> {
        let node = e.node();
        e.add_child::<Place<Xc>, Pc>(N::VALUE, node, key, start)
            .map(drop)
    }
    fn end<E: DriverOps<Q = Q>>(
        e: &mut E,
        key: &S::Key,
        reason: CK::EndReason,
    ) -> Result<(), EndError> {
        let id = end_target::<E, Xc, Pc>(e, key)?;
        e.end_typed::<Place<Xc>, Pc>(id, reason);
        Ok(())
    }
    fn replace<E: DriverOps<Q = Q>>(
        e: &mut E,
        key: S::Key,
        start: CK::Start,
        reason: CK::EndReason,
    ) -> Result<(), EndError> {
        let node = e.node();
        let id = end_target::<E, Xc, Pc>(e, &key)?;
        e.replace_typed::<Place<Xc>, Pc>(N::VALUE, node, id, key, start, reason);
        Ok(())
    }
}

/// The child of the place `Xc` under the editor's node kept under `key`,
/// when it can be ended: it is there, and no node links to it (a node
/// others link to ends after the last of them).
fn end_target<E, Xc, Pc>(e: &E, key: &PlaceKeyOf<E::Q, Xc>) -> Result<usize, EndError>
where
    E: DriverOps,
    Xc: ParentPath + PathFrom<RootOf<E::Q>> + PlaceKey<RootOf<E::Q>> + 'static,
    Pc: PlaceOps<E::Q, Place<Xc>>,
{
    let id = e
        .find_child::<Place<Xc>, Pc>(e.node(), key)
        .ok_or(EndError::NoChild {
            source: NoChildError,
        })?;
    if e.linked_to(id) {
        return Err(EndError::LinkedTo {
            source: LinkedToError,
        });
    }
    Ok(id)
}

/// The moves of the grandchildren under the children at the place `Xc`
/// whose positions are `Pc`, its children's places `Ix`.
#[allow(clippy::type_complexity)]
pub struct GrandchildPlaces<Xc, Pc, Ix>(PhantomData<fn() -> (Xc, Pc, Ix)>);
/// The move of a grandchild at the place `Xg`, positions `Pg`, under the
/// children at the place `Xc`, positions `Pc`.
#[allow(clippy::type_complexity)]
pub struct GrandchildPlace<Xc, Pc, Xg, Pg>(PhantomData<fn() -> (Xc, Pc, Xg, Pg)>);
impl<Q, S: ChildStorage, Xc, Pc> GrandchildEdits<Q, S, HNil> for GrandchildPlaces<Xc, Pc, HNil> {
    type Head = NoGrandchildPlace;
    type Tail = Self;
}
impl<Q, S, Xc, Pc, Xg, Pg, NG, R, GK, GS, T> GrandchildEdits<Q, S, HCons<Child<GK, GS>, T>>
    for GrandchildPlaces<Xc, Pc, HCons<ChildPlace<Xg, Pg, NG>, R>>
where
    S: ChildStorage,
    GK: NodeKind,
    GS: ChildStorage,
    T: ChildList,
    GrandchildPlace<Xc, Pc, Xg, Pg>: GrandchildEdit<Q, S, GS>,
    GrandchildPlaces<Xc, Pc, R>: GrandchildEdits<Q, S, T>,
{
    type Head = GrandchildPlace<Xc, Pc, Xg, Pg>;
    type Tail = GrandchildPlaces<Xc, Pc, R>;
}
impl<Q, S, G, Xc, Pc, Xg, Pg> GrandchildEdit<Q, S, G> for GrandchildPlace<Xc, Pc, Xg, Pg>
where
    Q: DriverComposition,
    S: ChildStorage,
    G: ChildStorage,
    Xc: ParentPath + PathFrom<RootOf<Q>> + PlaceKey<RootOf<Q>, Key = S::Key> + 'static,
    Xg: ParentPath + PathFrom<RootOf<Q>> + PlaceKey<RootOf<Q>, Key = G::Key> + 'static,
    Pc: PlaceOps<Q, Place<Xc>>,
    Pg: PlaceOps<Q, Place<Xg>>,
{
    fn move_to<E: DriverOps<Q = Q>>(
        e: &mut E,
        from: &S::Key,
        key: &G::Key,
        to: &S::Key,
    ) -> Result<(), MoveGrandchildError> {
        let node = e.node();
        let from =
            e.find_child::<Place<Xc>, Pc>(node, from)
                .ok_or(MoveGrandchildError::NoFrom {
                    source: NoChildError,
                })?;
        let moved =
            e.find_child::<Place<Xg>, Pg>(from, key)
                .ok_or(MoveGrandchildError::NoGrandchild {
                    source: NoChildError,
                })?;
        let to = e
            .find_child::<Place<Xc>, Pc>(node, to)
            .ok_or(MoveGrandchildError::NoTo {
                source: NoChildError,
            })?;
        e.move_node(moved, to)
            .map_err(|source| MoveGrandchildError::Occupied { source })
    }
}

// ---------------------------------------------------------------- links

/// Building, when a node of the kind `K` kept under a key `Key` is added,
/// the node it links to if that is not there.
pub trait LinkAdd<Q, K: NodeKind, Key> {
    /// The linked node, built from `start` when it was not there; `None`
    /// for no link.
    fn ensure<E: DriverOps<Q = Q>>(e: &mut E, key: &Key, start: &mut K::Start) -> Option<usize>;
}
impl<Q, K: NodeKind, Key> LinkAdd<Q, K, Key> for NoLinkPlace {
    fn ensure<E: DriverOps<Q = Q>>(_: &mut E, _: &Key, _: &mut K::Start) -> Option<usize> {
        None
    }
}
impl<Q, K, Key, Xb, Pb, N> LinkAdd<Q, K, Key> for LinkPlace<Xb, Pb, N>
where
    Q: DriverComposition,
    Xb: ParentPath + PathFrom<RootOf<Q>> + PlaceKey<RootOf<Q>, Key = Key> + 'static,
    K: LinkedStart<TargetOf<Q, Xb>>,
    Key: Clone,
    N: Number,
    Pb: PlaceOps<Q, Place<Xb>>,
{
    fn ensure<E: DriverOps<Q = Q>>(e: &mut E, key: &Key, start: &mut K::Start) -> Option<usize> {
        let root = e.root_node();
        if let Some(target) = e.find_child::<Place<Xb>, Pb>(root, key) {
            return Some(target);
        }
        let start = K::linked_start(start);
        // Invariant: the root is there while nodes are added, and no node
        // of the place is kept under the key, found just above.
        let target = e
            .add_child::<Place<Xb>, Pb>(N::VALUE, root, key.clone(), start)
            .expect("the linked node is built under the root, under a free key");
        Some(target)
    }
}

// ---------------------------------------------------------------- delivering

/// Delivering a sum of requests whose first alternative is for the place
/// at the number `I`: the node at the request's address, when it is of
/// that place, handles it with that place's editor.
pub trait Deliver<Q: DriverComposition, I>
where
    Cursor<I0>: CallWalk<Q>,
{
    /// Deliver it.
    fn deliver(self, d: &mut Driver<Q>);
}
impl<Q: DriverComposition, I> Deliver<Q, I> for CNil
where
    Cursor<I0>: CallWalk<Q>,
{
    fn deliver(self, _: &mut Driver<Q>) {
        match self {}
    }
}
impl<Q, I, K, T> Deliver<Q, I> for Coproduct<NodeRequest<RootOf<Q>, K>, T>
where
    Q: DriverComposition,
    K: NodeKind,
    I: Number,
    T: Deliver<Q, There<I>>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I>,
    for<'d> PlaceEdit<'d, Q, PlaceAt<Q, I>, PlacePositionsAt<Q, I>>: Editor<K>,
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
    fn deliver(self, d: &mut Driver<Q>) {
        match self {
            Coproduct::Inl(r) => {
                let Some(node) = d.find_key(&r.at) else {
                    return;
                };
                if d.live(node).map(|n| n.place) != Some(I::VALUE) {
                    return;
                }
                let mut edit = PlaceEdit::<Q, PlaceAt<Q, I>, PlacePositionsAt<Q, I>>::new(d, node);
                r.request.apply(Edit::new(&mut edit));
            }
            Coproduct::Inr(rest) => rest.deliver(d),
        }
    }
}

/// The operations of the end of a pack of places, never reached.
pub struct NoPlaceEdits;
const NO_PLACE: &str =
    "the end of a pack of places is never reached: the driver delivers to the places it has";
impl<Q, L: ChildList> ChildEdits<Q, L> for NoPlaceEdits {
    type Head = NoPlaceEdits;
    type Tail = NoPlaceEdits;
}
impl<Q, CK: NodeKind, S: ChildStorage> ChildEdit<Q, CK, S> for NoPlaceEdits {
    type Grandchildren = NoPlaceEdits;
    fn add<E: DriverOps<Q = Q>>(_: &mut E, _: S::Key, _: CK::Start) -> Result<(), AddError> {
        unreachable!("{NO_PLACE}")
    }
    fn end<E: DriverOps<Q = Q>>(_: &mut E, _: &S::Key, _: CK::EndReason) -> Result<(), EndError> {
        unreachable!("{NO_PLACE}")
    }
    fn replace<E: DriverOps<Q = Q>>(
        _: &mut E,
        _: S::Key,
        _: CK::Start,
        _: CK::EndReason,
    ) -> Result<(), EndError> {
        unreachable!("{NO_PLACE}")
    }
}
impl<Q, S: ChildStorage, L: ChildList> GrandchildEdits<Q, S, L> for NoPlaceEdits {
    type Head = NoPlaceEdits;
    type Tail = NoPlaceEdits;
}
impl<Q, S: ChildStorage, G: ChildStorage> GrandchildEdit<Q, S, G> for NoPlaceEdits {
    fn move_to<E: DriverOps<Q = Q>>(
        _: &mut E,
        _: &S::Key,
        _: &G::Key,
        _: &S::Key,
    ) -> Result<(), MoveGrandchildError> {
        unreachable!("{NO_PLACE}")
    }
}

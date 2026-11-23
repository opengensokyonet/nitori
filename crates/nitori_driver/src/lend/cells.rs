//! Splitting the tree around a node into disjoint cells.
//!
//! The node lent and the nodes on its way up are split component by
//! component; so is the linked node, and every node below them reached
//! through at-most-one children. A keyed store off the chain is one cell;
//! a keyed store on the chain is the node on the way (split further) and
//! one cell of the other children. Each cell is tagged with its place, the
//! positions of the children from it up to the root.
use core::{marker::PhantomData, ops::Add};

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};

use super::{FreezeOne, KeyedCell, LendError};
use crate::{
    need::LinkPos,
    structure::{
        AtMostOne, ByOwnKey, Child, ChildAt, ChildOfKind, ChildStorage, ChildStores, End, FlatMut,
        HandleAt, KeyAt, KeyedPath, KindFor, Node, NodeKind, SlotVec, SlotVecStore, Step, Store,
        StoreOf,
    },
};

/// A disjoint piece of what is lent, tagged by its place `Path`.
pub struct Cell<Path, T>(pub T, PhantomData<fn() -> Path>);
impl<Path, T> Cell<Path, T> {
    /// The cell holding `t`.
    pub fn new(t: T) -> Self {
        Self(t, PhantomData)
    }
}
impl<Path, T: Copy> Clone for Cell<Path, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<Path, T: Copy> Copy for Cell<Path, T> {}

/// A node's `&mut` components, each a cell tagged by the node's place.
pub trait Wrap<Path> {
    /// The cells.
    type Out;
    /// Tag the components.
    fn wrap(self) -> Self::Out;
}
impl<Path> Wrap<Path> for HNil {
    type Out = HNil;
    fn wrap(self) -> HNil {
        HNil
    }
}
impl<Path, H, T: Wrap<Path>> Wrap<Path> for HCons<H, T> {
    type Out = HCons<Cell<Path, H>, T::Out>;
    fn wrap(self) -> Self::Out {
        HCons {
            head: Cell::new(self.head),
            tail: self.tail.wrap(),
        }
    }
}
/// The components of a node that may be missing: every cell an `Option`.
pub trait WrapOpt<Path>: Sized {
    /// The cells.
    type Out;
    /// Tag the components, `None` each when the node is missing.
    fn wrap_opt(o: Option<Self>) -> Self::Out;
}
impl<Path> WrapOpt<Path> for HNil {
    type Out = HNil;
    fn wrap_opt(_: Option<HNil>) -> HNil {
        HNil
    }
}
impl<Path, H, T: WrapOpt<Path>> WrapOpt<Path> for HCons<H, T> {
    type Out = HCons<Cell<Path, Option<H>>, T::Out>;
    fn wrap_opt(o: Option<Self>) -> Self::Out {
        let (h, t) = o.map(|l| (l.head, l.tail)).unzip();
        HCons {
            head: Cell::new(h),
            tail: T::wrap_opt(t),
        }
    }
}

type FlatOf<'a, K, P> = <<K as KindFor<P>>::Data as FlatMut<'a>>::Out;

/// The cells of a node that may be missing (an at-most-one child of a node
/// lent): its components, and those of its own at-most-one children. Its
/// keyed children are not lent.
pub trait OptNodeCells<'a, P, Path>: KindFor<P> {
    /// The cells.
    type Out;
    /// Split the node, or give `None` cells.
    fn cells(n: Option<&'a mut Node<Self, P>>) -> Self::Out;
}
impl<'a, P: 'a, Path, K> OptNodeCells<'a, P, Path> for K
where
    K: KindFor<P>,
    K::Data: FlatMut<'a>,
    FlatOf<'a, K, P>: WrapOpt<Path>,
    K::Children: OptChildren<'a, P, Path, I0>,
    <FlatOf<'a, K, P> as WrapOpt<Path>>::Out:
        Add<<K::Children as OptChildren<'a, P, Path, I0>>::Out>,
{
    type Out = <<FlatOf<'a, K, P> as WrapOpt<Path>>::Out as Add<
        <K::Children as OptChildren<'a, P, Path, I0>>::Out,
    >>::Output;
    fn cells(n: Option<&'a mut Node<K, P>>) -> Self::Out {
        let (d, c) = match n {
            Some(Node { data, children, .. }) => (Some(data.flat_mut()), Some(children)),
            None => (None, None),
        };
        WrapOpt::wrap_opt(d) + K::Children::cells(c)
    }
}
/// The cells of the children of a node that may be missing, from the
/// position `Pos` on.
pub trait OptChildren<'a, P, Path, Pos>: ChildStores<P> {
    /// The cells.
    type Out;
    /// Split the stores, or give `None` cells.
    fn cells(stores: Option<&'a mut Self::Stores>) -> Self::Out;
}
impl<'a, P, Path, Pos> OptChildren<'a, P, Path, Pos> for HNil {
    type Out = HNil;
    fn cells(_: Option<&'a mut HNil>) -> HNil {
        HNil
    }
}
impl<'a, P: 'a, Path, Pos, CK, S, T> OptChildren<'a, P, Path, Pos> for HCons<Child<CK, S>, T>
where
    CK: KindFor<P>,
    S: OptStoreCells<'a, CK, P, HCons<Pos, Path>>,
    T: OptChildren<'a, P, Path, There<Pos>>,
    S::Out: Add<T::Out>,
{
    type Out = <S::Out as Add<T::Out>>::Output;
    fn cells(stores: Option<&'a mut Self::Stores>) -> Self::Out {
        let (h, t) = match stores {
            Some(HCons { head, tail }) => (Some(head), Some(tail)),
            None => (None, None),
        };
        S::cells(h) + T::cells(t)
    }
}
/// The cells of a store under a node that may be missing, by its storage
/// kind.
pub trait OptStoreCells<'a, CK: KindFor<P>, P, Path>: ChildStorage {
    /// The cells.
    type Out;
    /// Split the store, or give `None` cells.
    fn cells(s: Option<&'a mut StoreOf<Self, Node<CK, P>>>) -> Self::Out;
}
impl<'a, CK: OptNodeCells<'a, P, Path>, P: 'a, Path> OptStoreCells<'a, CK, P, Path> for AtMostOne {
    type Out = CK::Out;
    fn cells(s: Option<&'a mut Option<Node<CK, P>>>) -> Self::Out {
        CK::cells(s.and_then(|o| o.as_mut()))
    }
}
impl<'a, CK: KindFor<P>, P, Path, K: Clone + Eq + 'static> OptStoreCells<'a, CK, P, Path>
    for SlotVec<K>
{
    type Out = HNil;
    fn cells(_: Option<&'a mut SlotVecStore<K, Node<CK, P>>>) -> HNil {
        HNil
    }
}

/// What the children are told apart by in a keyed cell: every child of
/// the store ([`All`]), or every child but the one on the way up
/// ([`Others`]).
pub struct All;
/// See [`All`].
pub struct Others;

/// The cells of a store of children of a node lent, off the chain and not
/// linked, by its storage kind.
pub trait OffChain<'a, CK: KindFor<P>, P, Path>: ChildStorage {
    /// The cells.
    type Out;
    /// Split the store.
    fn cells(s: &'a mut StoreOf<Self, Node<CK, P>>) -> Self::Out;
}
impl<'a, CK: OptNodeCells<'a, P, Path>, P: 'a, Path> OffChain<'a, CK, P, Path> for AtMostOne {
    type Out = CK::Out;
    fn cells(s: &'a mut Option<Node<CK, P>>) -> Self::Out {
        CK::cells(s.as_mut())
    }
}
impl<'a, CK: KindFor<P>, P: 'a, Path, K: Clone + Eq + 'static> OffChain<'a, CK, P, Path>
    for SlotVec<K>
{
    type Out = HCons<Cell<Path, KeyedCell<'a, All, K, Node<CK, P>>>, HNil>;
    fn cells(s: &'a mut SlotVecStore<K, Node<CK, P>>) -> Self::Out {
        HCons {
            head: Cell::new(KeyedCell::new(&mut s.slots[..], &mut [])),
            tail: HNil,
        }
    }
}

/// A store on the chain, by its storage kind: the node on the way up goes
/// on being split, the other children of a keyed store are one more cell.
pub trait OnChain<'a, CK: KindFor<P>, P, Path>: ChildStorage {
    /// The cells of the other children.
    type Others;
    /// Split off the node at `at`, with the key it is kept under.
    #[allow(clippy::type_complexity)]
    fn split(
        s: &'a mut StoreOf<Self, Node<CK, P>>,
        at: Self::Handle,
    ) -> Option<(&'a Self::Key, &'a mut Node<CK, P>, Self::Others)>;
}
impl<'a, CK: KindFor<P>, P: 'a, Path> OnChain<'a, CK, P, Path> for AtMostOne {
    type Others = HNil;
    fn split(s: &'a mut Option<Node<CK, P>>, _: ()) -> Option<(&'a (), &'a mut Node<CK, P>, HNil)> {
        Some((&(), s.as_mut()?, HNil))
    }
}
impl<'a, CK: KindFor<P>, P: 'a, Path, K: Clone + Eq + 'static> OnChain<'a, CK, P, Path>
    for SlotVec<K>
{
    type Others = HCons<Cell<Path, KeyedCell<'a, Others, K, Node<CK, P>>>, HNil>;
    fn split(
        s: &'a mut SlotVecStore<K, Node<CK, P>>,
        at: usize,
    ) -> Option<(&'a K, &'a mut Node<CK, P>, Self::Others)> {
        if at >= s.slots.len() {
            return None;
        }
        let (before, rest) = s.slots.split_at_mut(at);
        let (own, after) = rest.split_first_mut()?;
        let (key, own) = own.as_mut()?;
        Some((
            key,
            own,
            HCons {
                head: Cell::new(KeyedCell::new(before, after)),
                tail: HNil,
            },
        ))
    }
}

/// No more of the chain below this node.
#[derive(Clone, Copy)]
pub struct NoChain;
/// No link to split here.
#[derive(Clone, Copy)]
pub struct NoLink;
/// The linked node: the root's child at the position `L`, at the handle.
pub struct LinkTo<L, H>(pub H, PhantomData<fn() -> L>);
impl<L, H: Copy> Clone for LinkTo<L, H> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<L, H: Copy> Copy for LinkTo<L, H> {}

/// A type-level `true`.
pub struct True;
/// A type-level `false`.
pub struct False;
/// Whether two frunk indices are the same.
pub trait IndexEq<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl IndexEq<I0> for I0 {
    type Out = True;
}
impl<I> IndexEq<There<I>> for I0 {
    type Out = False;
}
impl<I> IndexEq<I0> for There<I> {
    type Out = False;
}
impl<I: IndexEq<J>, J> IndexEq<There<J>> for There<I> {
    type Out = I::Out;
}
/// Whether the rest of the chain goes through the child at `Pos`.
pub trait ChainAtPos<Pos> {
    /// [`True`] or [`False`].
    type Out;
}
impl<Pos> ChainAtPos<Pos> for NoChain {
    type Out = False;
}
impl<Pos, I: IndexEq<Pos>, H, R> ChainAtPos<Pos> for Step<I, H, R> {
    type Out = I::Out;
}
/// Whether the link is to the child at `Pos`.
pub trait LinkAtPos<Pos> {
    /// [`True`] or [`False`].
    type Out;
}
impl<Pos> LinkAtPos<Pos> for NoLink {
    type Out = False;
}
impl<Pos, L: IndexEq<Pos>, H> LinkAtPos<Pos> for LinkTo<L, H> {
    type Out = L::Out;
}
/// A store neither on the chain nor linked.
pub struct RoleOther;
/// A store on the chain.
pub struct RoleChain;
/// The store of the linked node.
pub struct RoleLink;
/// The role of a store, from whether it is on the chain and linked.
pub trait RoleOf {
    /// One of the roles.
    type Role;
}
impl RoleOf for (False, False) {
    type Role = RoleOther;
}
impl RoleOf for (True, False) {
    type Role = RoleChain;
}
impl RoleOf for (False, True) {
    type Role = RoleLink;
}
type RoleAt<Ch, Lk, Pos> =
    <(<Ch as ChainAtPos<Pos>>::Out, <Lk as LinkAtPos<Pos>>::Out) as RoleOf>::Role;
type RoleOut<'a, CK, S, P, Pre, Pos, Ch, Lk, Kc> =
    <RoleAt<Ch, Lk, Pos> as RoleCells<'a, CK, S, P, Pre, Pos, Ch, Lk, Kc>>::Out;

/// What gets the cells of the store at one position, by the store's role;
/// `Kc` says whose key is lent ([`OwnKeySource`]).
pub trait RoleCells<'a, CK: KindFor<P>, S: ChildStorage, P, Pre, Pos, Ch, Lk, Kc> {
    /// The cells.
    type Out;
    /// Split the store.
    fn cells(
        s: &'a mut StoreOf<S, Node<CK, P>>,
        ch: Ch,
        lk: Lk,
        kc: Kc,
    ) -> Result<Self::Out, LendError>;
}
impl<'a, CK, S, P, Pre, Pos, Ch, Lk, Kc> RoleCells<'a, CK, S, P, Pre, Pos, Ch, Lk, Kc> for RoleOther
where
    CK: KindFor<P>,
    S: OffChain<'a, CK, P, HCons<Pos, Pre>>,
{
    type Out = S::Out;
    fn cells(s: &'a mut StoreOf<S, Node<CK, P>>, _: Ch, _: Lk, _: Kc) -> Result<S::Out, LendError> {
        Ok(S::cells(s))
    }
}
impl<'a, CK, S, P: 'a, Pre, Pos, H, R, Lk, Kc>
    RoleCells<'a, CK, S, P, Pre, Pos, Step<Pos, H, R>, Lk, Kc> for RoleChain
where
    CK: KindFor<P>,
    S: OnChain<'a, CK, P, HCons<Pos, Pre>, Handle = H>,
    Kc: OwnKeySource<'a>,
    R: ChainCells<'a, CK, P, HCons<Pos, Pre>, NoLink, Kc::Pick<S::Key>>,
    R::Out: Add<S::Others>,
{
    type Out = <R::Out as Add<S::Others>>::Output;
    fn cells(
        s: &'a mut StoreOf<S, Node<CK, P>>,
        ch: Step<Pos, H, R>,
        _: Lk,
        kc: Kc,
    ) -> Result<Self::Out, LendError> {
        let (key, n, others) = S::split(s, ch.handle).ok_or(LendError::NoNode)?;
        Ok(ch.rest.cells(n, NoLink, kc.pick(key))? + others)
    }
}
/// The linked node only; the rest of its store is not lent, nor its key.
impl<'a, CK, S, P: 'a, Pre, Pos, H, Ch, Kc>
    RoleCells<'a, CK, S, P, Pre, Pos, Ch, LinkTo<Pos, H>, Kc> for RoleLink
where
    CK: KindFor<P>,
    S: ChildStorage<Handle = H>,
    End: ChainCells<'a, CK, P, HCons<LinkPos<Pos>, Pre>, NoLink, FromSplit>,
{
    type Out = <End as ChainCells<'a, CK, P, HCons<LinkPos<Pos>, Pre>, NoLink, FromSplit>>::Out;
    fn cells(
        s: &'a mut StoreOf<S, Node<CK, P>>,
        _: Ch,
        lk: LinkTo<Pos, H>,
        _: Kc,
    ) -> Result<Self::Out, LendError> {
        // Invariant: the structure starts and ends a linked node together
        // with the nodes linking to it, so it is there whenever they are;
        // the link was found a moment ago.
        let n = s.get_mut(lk.0).expect(LINK_INVARIANT);
        End.cells(n, NoLink, FromSplit)
    }
}

/// The cells of all the children of a node lent, from the position `Pos`
/// on.
pub trait ChildrenCells<'a, P, Pre, Pos, Ch, Lk, Kc>: ChildStores<P> {
    /// The cells.
    type Out;
    /// Split the stores.
    fn cells(stores: &'a mut Self::Stores, ch: Ch, lk: Lk, kc: Kc) -> Result<Self::Out, LendError>;
}
impl<'a, P, Pre, Pos, Ch, Lk, Kc> ChildrenCells<'a, P, Pre, Pos, Ch, Lk, Kc> for HNil {
    type Out = HNil;
    fn cells(_: &'a mut HNil, _: Ch, _: Lk, _: Kc) -> Result<HNil, LendError> {
        Ok(HNil)
    }
}
impl<'a, P: 'a, Pre, Pos, Ch: Copy, Lk: Copy, Kc: Copy, CK, S, T>
    ChildrenCells<'a, P, Pre, Pos, Ch, Lk, Kc> for HCons<Child<CK, S>, T>
where
    CK: KindFor<P>,
    S: ChildStorage,
    T: ChildrenCells<'a, P, Pre, There<Pos>, Ch, Lk, Kc>,
    Ch: ChainAtPos<Pos>,
    Lk: LinkAtPos<Pos>,
    (Ch::Out, Lk::Out): RoleOf,
    RoleAt<Ch, Lk, Pos>: RoleCells<'a, CK, S, P, Pre, Pos, Ch, Lk, Kc>,
    RoleOut<'a, CK, S, P, Pre, Pos, Ch, Lk, Kc>: Add<T::Out>,
{
    type Out = <RoleOut<'a, CK, S, P, Pre, Pos, Ch, Lk, Kc> as Add<T::Out>>::Output;
    fn cells(stores: &'a mut Self::Stores, ch: Ch, lk: Lk, kc: Kc) -> Result<Self::Out, LendError> {
        let HCons { head, tail } = stores;
        let here = <RoleAt<Ch, Lk, Pos> as RoleCells<'a, CK, S, P, Pre, Pos, Ch, Lk, Kc>>::cells(
            head, ch, lk, kc,
        )?;
        Ok(here + T::cells(tail, ch, lk, kc)?)
    }
}

/// Where the key a lend's [`OwnKey`](crate::need::OwnKey) reads comes
/// from: the split of the chain ([`FromSplit`], for a node lent itself), or
/// a key given ([`GivenKey`], for the node a `build` builds). The node
/// lent last on the chain gets its cells ([`Cells`](OwnKeySource::Cells)).
pub trait OwnKeySource<'a>: Copy {
    /// The cell of the key, or none.
    type Cells;
    /// The source one level down, past a node kept under a key `K`.
    type Pick<K: 'a>: OwnKeySource<'a>;
    /// The cell.
    fn cells(self) -> Self::Cells;
    /// One level down, past a node kept under `key`.
    fn pick<K: 'a>(self, key: &'a K) -> Self::Pick<K>;
}
/// The key comes from the split: the root's is none.
#[derive(Clone, Copy)]
pub struct FromSplit;
/// The key of a node on the way down, from the split.
pub struct SplitKey<'a, K>(&'a K);
impl<K> Clone for SplitKey<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for SplitKey<'_, K> {}
/// The key given, whatever the split finds.
pub struct GivenKey<'a, K>(pub &'a K);
impl<K> Clone for GivenKey<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for GivenKey<'_, K> {}
/// The key a lend reads, as a cell.
pub struct OwnKeyRef<'a, K>(pub &'a K);
impl<K> Clone for OwnKeyRef<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for OwnKeyRef<'_, K> {}
impl<'a, K> FreezeOne for OwnKeyRef<'a, K> {
    type Out = Self;
    fn freeze(self) -> Self {
        self
    }
}
/// The place the key's cell is tagged with.
pub struct OwnKeyPlace;
/// The cell of the key a lend reads.
pub type OwnKeyCell<'a, K> = Cell<OwnKeyPlace, OwnKeyRef<'a, K>>;
impl<'a> OwnKeySource<'a> for FromSplit {
    type Cells = HNil;
    type Pick<K: 'a> = SplitKey<'a, K>;
    fn cells(self) -> HNil {
        HNil
    }
    fn pick<K: 'a>(self, key: &'a K) -> SplitKey<'a, K> {
        SplitKey(key)
    }
}
impl<'a, J: 'a> OwnKeySource<'a> for SplitKey<'a, J> {
    type Cells = HCons<OwnKeyCell<'a, J>, HNil>;
    type Pick<K: 'a> = SplitKey<'a, K>;
    fn cells(self) -> Self::Cells {
        HCons {
            head: Cell::new(OwnKeyRef(self.0)),
            tail: HNil,
        }
    }
    fn pick<K: 'a>(self, key: &'a K) -> SplitKey<'a, K> {
        SplitKey(key)
    }
}
impl<'a, J: 'a> OwnKeySource<'a> for GivenKey<'a, J> {
    type Cells = HCons<OwnKeyCell<'a, J>, HNil>;
    type Pick<K: 'a> = Self;
    fn cells(self) -> Self::Cells {
        HCons {
            head: Cell::new(OwnKeyRef(self.0)),
            tail: HNil,
        }
    }
    fn pick<K: 'a>(self, _: &'a K) -> Self {
        self
    }
}

type OwnCells<'a, Kc> = <Kc as OwnKeySource<'a>>::Cells;
type NodeCells<'a, K, P, Pre, Ch, Lk, Kc> = <<FlatOf<'a, K, P> as Wrap<Pre>>::Out as Add<
    <<K as NodeKind>::Children as ChildrenCells<'a, P, Pre, I0, Ch, Lk, Kc>>::Out,
>>::Output;

/// The cells of a node on the chain and of everything lent below it;
/// `self` is the rest of the path. `Kc` says whose key is lent: the node
/// lent last gets its cell.
pub trait ChainCells<'a, K: KindFor<P>, P, Pre, Lk, Kc = FromSplit>: Copy {
    /// The cells.
    type Out;
    /// Split the node.
    fn cells(self, n: &'a mut Node<K, P>, lk: Lk, kc: Kc) -> Result<Self::Out, LendError>;
}
impl<'a, K, P: 'a, Pre, Lk: Copy, Kc> ChainCells<'a, K, P, Pre, Lk, Kc> for End
where
    K: KindFor<P>,
    K::Data: FlatMut<'a>,
    FlatOf<'a, K, P>: Wrap<Pre>,
    Kc: OwnKeySource<'a>,
    K::Children: ChildrenCells<'a, P, Pre, I0, NoChain, Lk, Kc>,
    <FlatOf<'a, K, P> as Wrap<Pre>>::Out:
        Add<<K::Children as ChildrenCells<'a, P, Pre, I0, NoChain, Lk, Kc>>::Out>,
    OwnCells<'a, Kc>: Add<NodeCells<'a, K, P, Pre, NoChain, Lk, Kc>>,
{
    type Out = <OwnCells<'a, Kc> as Add<NodeCells<'a, K, P, Pre, NoChain, Lk, Kc>>>::Output;
    fn cells(self, n: &'a mut Node<K, P>, lk: Lk, kc: Kc) -> Result<Self::Out, LendError> {
        let Node { data, children, .. } = n;
        Ok(kc.cells() + (data.flat_mut().wrap() + K::Children::cells(children, NoChain, lk, kc)?))
    }
}
impl<'a, K, P: 'a, Pre, Lk: Copy, Kc: Copy, I, H: Copy, R: Copy> ChainCells<'a, K, P, Pre, Lk, Kc>
    for Step<I, H, R>
where
    K: KindFor<P>,
    K::Data: FlatMut<'a>,
    FlatOf<'a, K, P>: Wrap<Pre>,
    K::Children: ChildrenCells<'a, P, Pre, I0, Step<I, H, R>, Lk, Kc>,
    <FlatOf<'a, K, P> as Wrap<Pre>>::Out:
        Add<<K::Children as ChildrenCells<'a, P, Pre, I0, Step<I, H, R>, Lk, Kc>>::Out>,
{
    type Out = NodeCells<'a, K, P, Pre, Step<I, H, R>, Lk, Kc>;
    fn cells(self, n: &'a mut Node<K, P>, lk: Lk, kc: Kc) -> Result<Self::Out, LendError> {
        let Node { data, children, .. } = n;
        Ok(data.flat_mut().wrap() + K::Children::cells(children, self, lk, kc)?)
    }
}

/// The node the node lent links to, found read-only first: the links of
/// its kind (`Self`), from the root `R`, the path `X`; `LI` is inferred.
pub trait LinkSpec<R: KindFor<P>, P, X, LI> {
    /// The link, as the split takes it.
    type Lk: Copy;
    /// Find the linked node.
    fn find(root: &Node<R, P>, at: &X) -> Result<Self::Lk, LendError>;
}
impl<R: KindFor<P>, P, X> LinkSpec<R, P, X, ()> for HNil {
    type Lk = NoLink;
    fn find(_: &Node<R, P>, _: &X) -> Result<NoLink, LendError> {
        Ok(NoLink)
    }
}
impl<R, P, X, B: NodeKind, L> LinkSpec<R, P, X, L> for HCons<ByOwnKey<B>, HNil>
where
    R: KindFor<P>,
    R::Children: ChildOfKind<B, L> + ChildAt<L, P>,
    X: KeyedPath<R, P, (), Key = KeyAt<R::Children, L>>,
{
    type Lk = LinkTo<L, HandleAt<R::Children, L>>;
    fn find(root: &Node<R, P>, at: &X) -> Result<Self::Lk, LendError> {
        let key = at.key_of(root, &()).ok_or(LendError::NoNode)?;
        // Invariant: the structure starts and ends a linked node together
        // with the nodes linking to it, so it is there whenever they are.
        let h = <R::Children as ChildAt<L, P>>::store(&root.children)
            .find(key)
            .expect(LINK_INVARIANT);
        Ok(LinkTo(h, PhantomData))
    }
}

/// What a link promises: the node it points to is there while the node
/// linking to it is.
pub(crate) const LINK_INVARIANT: &str = "a node whose kind links to another node (`NodeKind::Linked`) \
     exists only while that node exists";

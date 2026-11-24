//! What the engine reads of the structure: the components of a kind, the
//! full keys, the chain from the root to a lend point, and the searches up
//! and down by component.
use core::marker::PhantomData;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::list::{Append, Concat, KeyTuple, PushKey, SameBranch, ShiftFirst};
use crate::{
    need::{KeyedBy, Optional, Pos, Shape, Single},
    structure::{
        AtMostOne, Child, ChildIndex, ChildStorage, KindAt, NodeKind, Shares, SlotVec, StorageAt,
    },
};

// ------------------------------------------------------------------ components

/// The components of one row, each `None` once taken: one `Option<&mut C>`
/// per component of the list `Self`.
pub trait RowFam: 'static {
    /// The row, borrowed for `'a`.
    type Row<'a>;
}
impl RowFam for HNil {
    type Row<'a> = HNil;
}
impl<C: 'static, T: RowFam> RowFam for HCons<C, T> {
    type Row<'a> = HCons<Option<&'a mut C>, T::Row<'a>>;
}

/// A node's data, one list of components per part, as one flat list.
pub trait FlatData: 'static {
    /// The flat list of the components' types.
    type Flat: RowFam;
    /// Every component, exclusive, as a row.
    fn row<'a>(&'a mut self) -> <Self::Flat as RowFam>::Row<'a>;
}
impl FlatData for HNil {
    type Flat = HNil;
    fn row(&mut self) -> HNil {
        HNil
    }
}
impl<D: PrependRow<Rest>, Rest: FlatData> FlatData for HCons<D, Rest> {
    type Flat = D::Flat;
    fn row<'a>(&'a mut self) -> <D::Flat as RowFam>::Row<'a> {
        let HCons { head, tail } = self;
        head.prepend(tail.row())
    }
}
/// One part's components put before the flat list of the parts after it.
pub trait PrependRow<Rest: FlatData>: 'static {
    /// The flat list of the components' types.
    type Flat: RowFam;
    /// The part's components, exclusive, before `rest`.
    fn prepend<'a>(
        &'a mut self,
        rest: <Rest::Flat as RowFam>::Row<'a>,
    ) -> <Self::Flat as RowFam>::Row<'a>;
}
impl<Rest: FlatData> PrependRow<Rest> for HNil {
    type Flat = Rest::Flat;
    fn prepend<'a>(
        &'a mut self,
        rest: <Rest::Flat as RowFam>::Row<'a>,
    ) -> <Rest::Flat as RowFam>::Row<'a> {
        rest
    }
}
impl<C: 'static, T: PrependRow<Rest>, Rest: FlatData> PrependRow<Rest> for HCons<C, T> {
    type Flat = HCons<C, T::Flat>;
    fn prepend<'a>(
        &'a mut self,
        rest: <Rest::Flat as RowFam>::Row<'a>,
    ) -> HCons<Option<&'a mut C>, <T::Flat as RowFam>::Row<'a>> {
        HCons {
            head: Some(&mut self.head),
            tail: self.tail.prepend(rest),
        }
    }
}

/// The flat list of the components of a node of the kind `K` for the
/// parts `Pt`.
pub type Comps<K, Pt> = <<Pt as Shares<K>>::Data as FlatData>::Flat;

/// A component `X` is in a list of components at the position `J`
/// (inferred).
pub trait Elem<X, J> {}
impl<X, T> Elem<X, Here> for HCons<X, T> {}
impl<X, H, T: Elem<X, J>, J> Elem<X, There<J>> for HCons<H, T> {}

/// A kind whose components are `Cs` has `Self`, a component or all the
/// components of a list ([`AllOf`](super::members::AllOf)), at the
/// positions `J` (inferred).
///
/// What is looked for is `Self`, so that, not known yet, it leaves the
/// search undecided rather than searching every list.
pub trait FoundIn<Cs, J> {}
impl<X: crate::id::Numbered, Cs: Elem<X, J>, J> FoundIn<Cs, J> for X {}

// ------------------------------------------------------------------ storage kinds

/// What a storage kind adds to the full key: nothing for at most one
/// child, the child's key for children by key.
pub trait FkStep<F>: ChildStorage {
    /// The child's full key.
    type Out: Clone + PartialEq + 'static;
    /// The child's full key from its parent's and its own key.
    fn push(parent: &F, key: &Self::Key) -> Self::Out;
}
impl<F: Clone + PartialEq + 'static> FkStep<F> for AtMostOne {
    type Out = F;
    fn push(parent: &F, _: &()) -> F {
        parent.clone()
    }
}
impl<F, K> FkStep<F> for SlotVec<K>
where
    F: PushKey<K>,
    F::Out: Clone + PartialEq + 'static,
    K: Clone + Eq + 'static,
{
    type Out = F::Out;
    fn push(parent: &F, key: &K) -> F::Out {
        parent.push_key(key.clone())
    }
}

/// The first level of what is left of a full key `F`, read from the root
/// down: the key of a child of this storage kind and the levels below it.
pub trait SplitKey<F>: ChildStorage {
    /// The levels below.
    type Rest;
    /// The child's key and the levels below.
    fn split_key(full: &F) -> (Self::Key, &Self::Rest);
}
impl<F> SplitKey<F> for AtMostOne {
    type Rest = F;
    fn split_key(full: &F) -> ((), &F) {
        ((), full)
    }
}
impl<K: Clone + Eq + 'static, R> SplitKey<HCons<K, R>> for SlotVec<K> {
    type Rest = R;
    fn split_key(full: &HCons<K, R>) -> (K, &R) {
        (full.head.clone(), &full.tail)
    }
}

/// The storage kind of the lend point's own level when it is the root,
/// which no parent keeps: it has no key.
pub struct NoParent;

// ------------------------------------------------------------------ routes

/// The kind `L` at or below the kind `Self`, at the route `Route` (a list
/// of positions among children); `I` is inferred.
#[diagnostic::on_unimplemented(
    message = "the structure has no kind `{L}` at or below `{Self}`",
    label = "not a kind of this structure"
)]
pub trait Locate<L, I> {
    /// The positions among children from `Self` down to `L`.
    type Route;
}
/// The kind is the one searched from.
pub struct IsSelf;
/// The kind is below one of the children.
pub struct InKid<I>(PhantomData<fn() -> I>);
/// The first child of a list.
pub struct KHead<I>(PhantomData<fn() -> I>);
/// A child after the first.
pub struct KTail<I>(PhantomData<fn() -> I>);
impl<L: NodeKind> Locate<L, IsSelf> for L {
    type Route = HNil;
}
impl<K: NodeKind, L, I> Locate<L, InKid<I>> for K
where
    K::Children: LocateIn<L, I>,
{
    type Route = <K::Children as LocateIn<L, I>>::Route;
}
/// The kind `L` below a list of children.
pub trait LocateIn<L, I> {
    /// The route, from the list's first position.
    type Route;
}
impl<L, C: Locate<L, I>, St, T, I> LocateIn<L, KHead<I>> for HCons<Child<C, St>, T> {
    type Route = HCons<Here, C::Route>;
}
impl<L, H, T: LocateIn<L, I>, I> LocateIn<L, KTail<I>> for HCons<H, T>
where
    T::Route: ShiftFirst,
{
    type Route = <T::Route as ShiftFirst>::Out;
}

// ------------------------------------------------------------------ chain

/// A node on the chain from the root to a lend point: its kind `K`, its
/// components `Cs`, its full key `F`, the route `Pre` from the root to it,
/// and the storage kind `St` its parent keeps it by.
pub struct On<K, Cs, F, Pre, St>(PhantomData<fn() -> (K, Cs, F, Pre, St)>);

/// The chain from a node of the kind `K` down the route `R` (`Self` is
/// `(K, R)`), for the parts `Pt`; the node has the full key `F`, is at the
/// route `Pre` from the root and is kept by `St`.
pub trait ChainOf<Pt, F, Pre, St> {
    /// The nodes, the first first.
    type Chain;
    /// The full key of the last node.
    type Fk: Clone + PartialEq + 'static;
    /// The kind of the last node.
    type Kind: NodeKind;
    /// The storage kind the last node is kept by.
    type Storage;
}
impl<K: NodeKind, Pt, F, Pre, St> ChainOf<Pt, F, Pre, St> for (K, HNil)
where
    Pt: Shares<K>,
    Pt::Data: FlatData,
    F: Clone + PartialEq + 'static,
{
    type Chain = HCons<On<K, Comps<K, Pt>, F, Pre, St>, HNil>;
    type Fk = F;
    type Kind = K;
    type Storage = St;
}
#[allow(clippy::type_complexity)]
impl<K: NodeKind, Pt, F, Pre, St, J, R> ChainOf<Pt, F, Pre, St> for (K, HCons<J, R>)
where
    Pt: Shares<K>,
    Pt::Data: FlatData,
    K::Children: ChildIndex<J>,
    StorageAt<K::Children, J>: FkStep<F>,
    Pre: Append<HCons<J, HNil>>,
    (KindAt<K::Children, J>, R): ChainOf<
            Pt,
            <StorageAt<K::Children, J> as FkStep<F>>::Out,
            <Pre as Append<HCons<J, HNil>>>::Out,
            StorageAt<K::Children, J>,
        >,
{
    type Chain = HCons<
        On<K, Comps<K, Pt>, F, Pre, St>,
        <(KindAt<K::Children, J>, R) as ChainOf<
            Pt,
            <StorageAt<K::Children, J> as FkStep<F>>::Out,
            <Pre as Append<HCons<J, HNil>>>::Out,
            StorageAt<K::Children, J>,
        >>::Chain,
    >;
    type Fk = <(KindAt<K::Children, J>, R) as ChainOf<
        Pt,
        <StorageAt<K::Children, J> as FkStep<F>>::Out,
        <Pre as Append<HCons<J, HNil>>>::Out,
        StorageAt<K::Children, J>,
    >>::Fk;
    type Kind = <(KindAt<K::Children, J>, R) as ChainOf<
        Pt,
        <StorageAt<K::Children, J> as FkStep<F>>::Out,
        <Pre as Append<HCons<J, HNil>>>::Out,
        StorageAt<K::Children, J>,
    >>::Kind;
    type Storage = <(KindAt<K::Children, J>, R) as ChainOf<
        Pt,
        <StorageAt<K::Children, J> as FkStep<F>>::Out,
        <Pre as Append<HCons<J, HNil>>>::Out,
        StorageAt<K::Children, J>,
    >>::Storage;
}

/// A lend point as types: the root kind `R`, the parts `Pt`, the route
/// `Rt` from the root, the depth `Sd` of the members being lent (zero
/// outside an `Under`), above which nothing may be taken exclusive, and the
/// type `Xc` of the lending's context.
pub struct Cx<R, Pt, Rt, Sd, Xc>(PhantomData<fn() -> (R, Pt, Rt, Sd, Xc)>);

/// What a lend point gives the needs taken at it.
pub trait Ctx {
    /// The root kind.
    type Root: NodeKind;
    /// The parts.
    type Parts;
    /// The route from the root.
    type Route;
    /// The depth of the members being lent.
    type Depth;
    /// The chain from the root to the lend point, the root first.
    type Chain;
    /// The lend point's full key.
    type Fk: Clone + PartialEq + 'static;
    /// The lend point's kind.
    type Kind: NodeKind;
    /// The storage kind the lend point is kept by ([`NoParent`] for the
    /// root).
    type Storage;
    /// The context of the lending, which the context needs read.
    type Context;
}
impl<R: NodeKind, Pt, Rt, Sd, Xc> Ctx for Cx<R, Pt, Rt, Sd, Xc>
where
    (R, Rt): ChainOf<Pt, HNil, HNil, NoParent>,
{
    type Root = R;
    type Parts = Pt;
    type Route = Rt;
    type Depth = Sd;
    type Chain = <(R, Rt) as ChainOf<Pt, HNil, HNil, NoParent>>::Chain;
    type Fk = <(R, Rt) as ChainOf<Pt, HNil, HNil, NoParent>>::Fk;
    type Kind = <(R, Rt) as ChainOf<Pt, HNil, HNil, NoParent>>::Kind;
    type Storage = <(R, Rt) as ChainOf<Pt, HNil, HNil, NoParent>>::Storage;
    type Context = Xc;
}

// ------------------------------------------------------------------ up

/// The node of the chain at the position `I` from the root is the one
/// that has `X` (inferred).
pub trait FindUp<X, I> {
    /// Its kind.
    type Kind: NodeKind;
    /// Its full key.
    type Fk: Clone + PartialEq + 'static;
    /// The route from the root to it.
    type Pre;
    /// The position of `X` among its components.
    type Col;
}
/// The first node of the chain has the component, at `J`.
pub struct Found<J>(PhantomData<fn() -> J>);
/// A node further down the chain has it.
pub struct Next<I>(PhantomData<fn() -> I>);
impl<X, K: NodeKind, Cs: Elem<X, J>, F, Pre, St, T, J> FindUp<X, Found<J>>
    for HCons<On<K, Cs, F, Pre, St>, T>
where
    F: Clone + PartialEq + 'static,
{
    type Kind = K;
    type Fk = F;
    type Pre = Pre;
    type Col = J;
}
impl<X, H, T: FindUp<X, I>, I> FindUp<X, Next<I>> for HCons<H, T> {
    type Kind = T::Kind;
    type Fk = T::Fk;
    type Pre = T::Pre;
    type Col = T::Col;
}

// ------------------------------------------------------------------ down

/// The members are the anchor itself.
pub struct ShSelf;
/// Every level down to the members keeps at most one child.
pub struct ShOpt;
/// Some level down to the members keeps children by key; `KL` are the
/// keys of those levels, the first first.
pub struct ShKeys<KL>(PhantomData<fn() -> KL>);

/// The public shape of a result.
pub trait ToShape: 'static {
    /// The shape.
    type Shape: Shape;
}
impl ToShape for ShSelf {
    type Shape = Single;
}
impl ToShape for ShOpt {
    type Shape = Optional;
}
impl<KL: KeyTuple + 'static> ToShape for ShKeys<KL> {
    type Shape = KeyedBy<KL::Tuple>;
}

/// A storage kind laid over the shape of the levels below it.
pub trait Compose<Inner> {
    /// The shape from one level higher.
    type Out;
}
impl Compose<ShSelf> for AtMostOne {
    type Out = ShOpt;
}
impl Compose<ShOpt> for AtMostOne {
    type Out = ShOpt;
}
impl<KL> Compose<ShKeys<KL>> for AtMostOne {
    type Out = ShKeys<KL>;
}
impl<K> Compose<ShSelf> for SlotVec<K> {
    type Out = ShKeys<HCons<K, HNil>>;
}
impl<K> Compose<ShOpt> for SlotVec<K> {
    type Out = ShKeys<HCons<K, HNil>>;
}
impl<K, KL> Compose<ShKeys<KL>> for SlotVec<K> {
    type Out = ShKeys<HCons<K, KL>>;
}

/// The kind at or below `Self` that has the component `X` (inferred `I`),
/// for the parts `Pt`.
pub trait FindBelow<Pt, X, I> {
    /// The route from `Self` down to it.
    type Rel;
    /// The position of `X` among its components.
    type Col;
    /// The shape of the result.
    type Sh;
}
/// The kind searched from has the component, at `J`.
pub struct AtSelf<J>(PhantomData<fn() -> J>);
/// A kind below one of the children has it.
pub struct Down<I>(PhantomData<fn() -> I>);
impl<Pt, X, K: NodeKind, J> FindBelow<Pt, X, AtSelf<J>> for K
where
    Pt: Shares<K>,
    Pt::Data: FlatData,
    X: FoundIn<Comps<K, Pt>, J>,
{
    type Rel = HNil;
    type Col = J;
    type Sh = ShSelf;
}
impl<Pt, X, K: NodeKind, I> FindBelow<Pt, X, Down<I>> for K
where
    K::Children: FindBelowIn<Pt, X, I>,
{
    type Rel = <K::Children as FindBelowIn<Pt, X, I>>::Rel;
    type Col = <K::Children as FindBelowIn<Pt, X, I>>::Col;
    type Sh = <K::Children as FindBelowIn<Pt, X, I>>::Sh;
}
/// The kind that has `X` below a list of children.
pub trait FindBelowIn<Pt, X, I> {
    /// The route, from the list's first position.
    type Rel;
    /// The position of `X` among its components.
    type Col;
    /// The shape of the result.
    type Sh;
}
impl<Pt, X, C: FindBelow<Pt, X, I>, St: Compose<C::Sh>, T, I> FindBelowIn<Pt, X, KHead<I>>
    for HCons<Child<C, St>, T>
{
    type Rel = HCons<Here, C::Rel>;
    type Col = C::Col;
    type Sh = St::Out;
}
impl<Pt, X, H, T: FindBelowIn<Pt, X, I>, I> FindBelowIn<Pt, X, KTail<I>> for HCons<H, T>
where
    T::Rel: ShiftFirst,
{
    type Rel = <T::Rel as ShiftFirst>::Out;
    type Col = T::Col;
    type Sh = T::Sh;
}

/// How the members of a result are gathered, by its shape, from the full
/// keys `rows` of the nodes of the members' kind: the anchor's full key is
/// `AF`, a member's `MF`, the lend point's `LF`.
pub trait Collect<AF, MF, LF>: ToShape {
    /// The result, `f` called for each member; `line` is whether the
    /// members' kind and the lend point's are on one line from the root
    /// ([`RouteLine`](super::list::RouteLine)), without which no member is
    /// in the lend point's own branch.
    fn collect<V>(
        rows: &[&MF],
        anchor: &AF,
        at: &LF,
        line: bool,
        f: impl FnMut(&MF) -> V,
    ) -> <Self::Shape as Shape>::Out<V>;
}
impl<AF, LF> Collect<AF, AF, LF> for ShSelf {
    fn collect<V>(
        _: &[&AF],
        anchor: &AF,
        _: &LF,
        _: bool,
        mut f: impl FnMut(&AF) -> V,
    ) -> crate::need::SingleView<V> {
        // The anchor is on the chain of the lend point, which is there.
        crate::need::SingleView::new(f(anchor))
    }
}
impl<AF: PartialEq, LF> Collect<AF, AF, LF> for ShOpt {
    fn collect<V>(
        rows: &[&AF],
        anchor: &AF,
        _: &LF,
        _: bool,
        mut f: impl FnMut(&AF) -> V,
    ) -> Option<V> {
        rows.contains(&anchor).then(|| f(anchor))
    }
}
impl<AF: Concat<KL>, KL: KeyTuple + 'static, LF> Collect<AF, AF::Out, LF> for ShKeys<KL>
where
    AF::Out: SameBranch<LF>,
{
    fn collect<V>(
        rows: &[&AF::Out],
        anchor: &AF,
        at: &LF,
        line: bool,
        mut f: impl FnMut(&AF::Out) -> V,
    ) -> crate::need::KeyedView<KL::Tuple, V> {
        crate::need::KeyedView::new(
            rows.iter()
                .filter_map(|r| {
                    AF::strip(r, anchor)
                        .map(|keys| (keys.to_tuple(), f(r), line && r.same_branch(at)))
                })
                .collect(),
        )
    }
}

// ------------------------------------------------------------------ positions

/// The position of a need taken on the way up ([`Mut`](crate::need::Mut),
/// [`Ref`](crate::need::Ref)); `I` is where it was found.
pub struct UpPos<I>(PhantomData<fn() -> I>);
impl<I: 'static> Pos for UpPos<I> {}

/// The position of a need taken at each member of an `Under`, on the
/// member itself; `J` is the position of the component.
pub struct MemberPos<J>(PhantomData<fn() -> J>);
impl<J: 'static> Pos for MemberPos<J> {}

/// The position of an `At`: the new lend point found at `I`, the inner
/// need at `P`.
pub struct AtPos<I, P>(PhantomData<fn() -> (I, P)>);
impl<I: 'static, P: Pos> Pos for AtPos<I, P> {
    type Inner = P;
}
/// The new lend point of an `At` inside an `Under` is the member.
pub struct AtMember;

/// The position of an `Under`: the anchor found at `IA`, the members' kind
/// at `IK` below it, the shape `Sh`, and the members' need at `P`.
pub struct UnderPos<IA, IK, Sh, P>(PhantomData<fn() -> (IA, IK, Sh, P)>);
impl<IA: 'static, IK: 'static, Sh: ToShape, P: Pos> Pos for UnderPos<IA, IK, Sh, P> {
    type Shape = Sh::Shape;
    type Inner = P;
}

/// The position of an [`OwnKey`](crate::need::OwnKey).
pub struct KeyPos;
impl Pos for KeyPos {}

/// The position of an [`OwnAddress`](crate::need::OwnAddress) lent as `A`.
pub struct AddressPos<A>(PhantomData<fn() -> A>);
impl<A: 'static> Pos for AddressPos<A> {
    type Address = A;
}

//! The places of a tree: a path type for each, found by the kinds alone, and
//! the check that no kind is at two places.
use core::{any::TypeId, marker::PhantomData, ops::Add};

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};

use crate::{
    call::RootOf,
    structure::{
        Append, Child, ChildIndex, ChildStorage, End, KeyAt, LastChildren, NodeKind, ParentPath,
        PathFrom, SplitLast, Step,
    },
};

/// The nodes a path type `X` from the root leads to.
pub struct Place<X>(PhantomData<fn() -> X>);
/// The end of a list of places, a filler the driver never reaches.
pub struct NoPlace;

/// The key the nodes at a path type `Self` from a root of the kind `R`
/// are kept under; the root has none (`()`, never lent).
pub trait PlaceKey<R: NodeKind> {
    /// The key.
    type Key: 'static;
}
impl<R: NodeKind> PlaceKey<R> for End {
    type Key = ();
}
impl<R: NodeKind, I, H, Rr> PlaceKey<R> for Step<I, H, Rr>
where
    Self: SplitLast<Parent: PathFrom<R>>,
    LastChildren<R, Self>: ChildIndex<<Self as SplitLast>::Index>,
{
    type Key = KeyAt<LastChildren<R, Self>, <Self as SplitLast>::Index>;
}
/// The key of the nodes at `X` of `Q`'s tree.
pub type PlaceKeyOf<Q, X> = <X as PlaceKey<RootOf<Q>>>::Key;

/// What a place of a tree whose root is of the kind `R` is: its path
/// type.
pub trait PlaceDesc<R: NodeKind> {
    /// The path type.
    type At: ParentPath + PathFrom<R> + PlaceKey<R> + 'static;
}
impl<R: NodeKind, X: ParentPath + PathFrom<R> + PlaceKey<R> + 'static> PlaceDesc<R> for Place<X> {
    type At = X;
}
impl<R: NodeKind> PlaceDesc<R> for NoPlace {
    type At = End;
}

/// Every place of the kind `Self` at the path type `X` and below it, in
/// tree order (a node's place before the places below it).
pub trait PlacesUnder<X> {
    /// The places.
    type Out;
}
impl<K: NodeKind, X> PlacesUnder<X> for K
where
    K::Children: ChildPlaces<X, I0>,
{
    type Out = HCons<Place<X>, <K::Children as ChildPlaces<X, I0>>::Out>;
}
/// The places below a node at `X`, from its child at `Pos` on.
pub trait ChildPlaces<X, Pos> {
    /// The places.
    type Out;
}
impl<X, Pos> ChildPlaces<X, Pos> for HNil {
    type Out = HNil;
}
impl<X, Pos, CK, S, T> ChildPlaces<X, Pos> for HCons<Child<CK, S>, T>
where
    S: ChildStorage,
    X: Append<Pos, S::Handle>,
    CK: PlacesUnder<X::Out>,
    T: ChildPlaces<X, There<Pos>>,
    CK::Out: Add<T::Out>,
{
    type Out = <CK::Out as Add<T::Out>>::Output;
}
/// Every place of the tree of `Q`, in tree order; the root's is first.
pub type PlacesOf<Q> = <RootOf<Q> as PlacesUnder<End>>::Out;

/// Whether `A` and `B` are one type, at compile time.
const fn same_type<A: 'static, B: 'static>() -> bool {
    TypeId::of::<A>() == TypeId::of::<B>()
}
/// Every kind of the places `Self` of a tree whose root is of the kind `R`
/// is at one place only: a kind is not a child in two places. The
/// composition evaluates [`CHECK`](KindsOnce::CHECK); a kind at two places
/// fails it, and the error names the list and the kind.
pub trait KindsOnce<R: NodeKind> {
    /// Evaluates only when every kind is at one place.
    const CHECK: ();
}
impl<R: NodeKind> KindsOnce<R> for HNil {
    const CHECK: () = ();
}
impl<R: NodeKind, X: PathFrom<R>, T: KindsOnce<R> + KindNotIn<R, X::Target>> KindsOnce<R>
    for HCons<Place<X>, T>
{
    const CHECK: () = {
        let () = <T as KindNotIn<R, X::Target>>::CHECK;
        <T as KindsOnce<R>>::CHECK
    };
}
/// The kind `K` is at none of the places `Self`.
pub trait KindNotIn<R: NodeKind, K> {
    /// Evaluates only when `K` is at none of them.
    const CHECK: ();
}
impl<R: NodeKind, K> KindNotIn<R, K> for HNil {
    const CHECK: () = ();
}
impl<R: NodeKind, K: 'static, X: PathFrom<R>, T: KindNotIn<R, K>> KindNotIn<R, K>
    for HCons<Place<X>, T>
{
    const CHECK: () = {
        assert!(
            !same_type::<K, X::Target>(),
            "a node kind is at two places of the structure: give each place its own kind"
        );
        T::CHECK
    };
}

/// A list of places as head and tail (the empty list: [`NoPlace`] and
/// itself).
pub trait PlaceShape<R: NodeKind> {
    /// The first place.
    type Head: PlaceDesc<R>;
    /// The others.
    type Tail: PlaceShape<R>;
}
impl<R: NodeKind> PlaceShape<R> for HNil {
    type Head = NoPlace;
    type Tail = HNil;
}
impl<R: NodeKind, H: PlaceDesc<R>, T: PlaceShape<R>> PlaceShape<R> for HCons<H, T> {
    type Head = H;
    type Tail = T;
}
/// The place at the position `I` of a list.
pub trait NthPlace<R: NodeKind, I>: PlaceShape<R> {
    /// The place.
    type Item: PlaceDesc<R>;
}
impl<R: NodeKind, L: PlaceShape<R>> NthPlace<R, I0> for L {
    type Item = L::Head;
}
impl<R: NodeKind, L: PlaceShape<R>, I> NthPlace<R, There<I>> for L
where
    L::Tail: NthPlace<R, I>,
{
    type Item = <L::Tail as NthPlace<R, I>>::Item;
}
/// The place of the path type `X` is at the position `I`.
pub trait FindPlace<X, I> {}
impl<X, T> FindPlace<X, I0> for HCons<Place<X>, T> {}
impl<X, H, T: FindPlace<X, I>, I> FindPlace<X, There<I>> for HCons<H, T> {}

//! The places of a tree, the components at each, and the proof that every
//! hook of every component can be lent at its place.
//!
//! A place is a path type from the root ([`Place<X>`](Place)): every node
//! of the kind reached that way is there. The places follow from the
//! kinds alone ([`PlacesOf`]). At each place every part's share gives
//! components; each component's hooks are lent [`OwnNeeds`] at the place
//! and its `build` the [`Hooks::BuildNeeds`] at the parent's place, and its
//! subscriptions are resolved at the place. [`composition!`](crate::composition)
//! proves them all once, into the opaque [`DriverComposition::Places`].
use alloc::vec::Vec;
use core::{any::TypeId, marker::PhantomData, ops::Add, task::Waker};

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};
use nitori_clock::MonotonicClock;

use crate::{
    call::{ChainAt, Composition, LendInstant, OutboxOf, PartsOf, RootOf},
    event::{NodeEnding, NodeStarted, Outbox, OwnPlace, Subscription, Subscriptions, Triggered},
    hooks::{
        AbortCause, ArgsOf, BuildKeyed, BuildView, EndCause, EndReasonOf, HookKeys, HookView,
        Hooks, OwnNeeds, PartList, StartOf, Timing,
    },
    lend::{Flat, FromSplit, GivenKey, KeyedList, LendAt, LendError, NeedSet, ViewOf, Writes},
    need::ChainOf,
    request::{IntoRequestSum, PlaceRequests, RequestSumOf},
    structure::{
        Address, Append, Child, ChildAt, ChildIndex, ChildStorage, End, FromAddress, KeyAt,
        KindFor, LastChildren, MoveError, Node, NodeKind, NodePath, ParentPath, PathFrom,
        RemainsList, RemainsOf, Share, Shares, SplitLast, Step, Store, move_to, remove,
    },
};

// ---------------------------------------------------------------- the places

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

// ---------------------------------------------------------------- the composition

/// A list of parts whose arguments are for itself.
pub trait SelfParts: PartList<Self> + Sized + 'static {}
impl<L: PartList<L> + 'static> SelfParts for L {}

/// A composition a driver runs: besides the calls, every hook of every
/// component proved at its place, the driver's clock, and the sum of the
/// requests.
///
/// Written with [`composition!`](crate::composition)`(impl driver for Q:
/// Root, Parts, Clock)`; the places are opaque, defined by
/// [`places`](DriverComposition::places).
pub trait DriverComposition:
    Composition<
        Parts: SelfParts,
        Root: PlacesUnder<
            End,
            Out: PlaceShape<<Self as Composition>::Root>
                     + PlaceRequests<<Self as Composition>::Root>,
        >,
    >
{
    /// The clock the driver reads and sleeps on; every component with a
    /// deadline has its instant.
    type Clock: MonotonicClock<Instant = LendInstant<Self>> + 'static;
    /// What the parts keep of the root's shares after it ended
    /// ([`RemainsOf`]`<Parts, Root>`).
    type Remains: Default + 'static;
    /// The positions of every hook of every place.
    type Places: PlacePack<Self, PlacesOf<Self>>;
    /// How what each call yields goes into the requests.
    type Yields: super::walk::YieldPack<Self, crate::call::CallSlotsOf<Self>>;
    /// Only to define `Places`.
    fn places() -> PhantomData<Self::Places>;
    /// Only to define `Yields`.
    fn yields() -> PhantomData<Self::Yields>;
}
/// The instant of the clock `C`.
pub type ClockInstantOf<C> = <C as MonotonicClock>::Instant;
/// The clock of `Q`.
pub type ClockOf<Q> = <Q as DriverComposition>::Clock;
/// The instant of the clock of `Q`.
pub type InstantOf<Q> = <ClockOf<Q> as MonotonicClock>::Instant;
/// The sum of the requests of `Q`'s tree.
pub type RequestsOf<Q> = RequestSumOf<RootOf<Q>>;
/// The root of `Q`'s tree.
pub type RootNode<Q> = Node<RootOf<Q>, PartsOf<Q>>;
/// The address of a node of `Q`'s tree.
pub type AddressOf<Q> = Address<RootOf<Q>>;
/// The kind at the path type `X` of `Q`'s tree.
pub type TargetOf<Q, X> = <X as PathFrom<RootOf<Q>>>::Target;
/// The path of the parent of the nodes at `X`.
pub type ParentOf<X> = <X as ParentPath>::Parent;

// ---------------------------------------------------------------- one component

/// Lending what a `build` reads at the parent of a node at the path
/// `Self`, for the needs `N`, with the instant `T`; the new node's key is
/// what [`OwnKey`](crate::need::OwnKey) reads. At the root there is no
/// parent and `N` is empty.
pub trait BuildAt<'a, R: KindFor<P>, P, N: NeedSet<P>, I, T = ()>:
    ParentPath + PlaceKey<R>
{
    /// The keyed positions.
    type Positions: KeyedList<P, <N as Flat>::Keyed>;
    /// Lend the parent at `parent` to `f`, the new node's key `key`.
    fn lend<O>(
        root: Option<&'a mut Node<R, P>>,
        parent: &Self::Parent,
        key: &'a Self::Key,
        outbox: &'a Outbox<T>,
        f: impl FnOnce(ViewOf<'a, N, P, Self::Positions>) -> O,
    ) -> Result<O, LendError>;
}
impl<'a, R: KindFor<P>, P, T> BuildAt<'a, R, P, HNil, (), T> for End {
    type Positions = HNil;
    fn lend<O>(
        _: Option<&'a mut Node<R, P>>,
        _: &(),
        _: &'a (),
        _: &'a Outbox<T>,
        f: impl FnOnce(HNil) -> O,
    ) -> Result<O, LendError> {
        Ok(f(HNil))
    }
}
impl<'a, R: KindFor<P>, P, N, I, J, H, Rr, T> BuildAt<'a, R, P, N, I, T> for Step<J, H, Rr>
where
    Step<J, H, Rr>: ParentPath + PlaceKey<R>,
    N: LendAt<'a, R, P, ParentOf<Step<J, H, Rr>>, I, T, GivenKey<'a, PlaceKeyR<R, Self>>>,
{
    type Positions = N::Positions;
    fn lend<O>(
        root: Option<&'a mut Node<R, P>>,
        parent: &ParentOf<Self>,
        key: &'a PlaceKeyR<R, Self>,
        outbox: &'a Outbox<T>,
        f: impl FnOnce(ViewOf<'a, N, P, Self::Positions>) -> O,
    ) -> Result<O, LendError> {
        N::lend(
            root.ok_or(LendError::NoNode)?,
            *parent,
            outbox,
            GivenKey(key),
            f,
        )
    }
}
type PlaceKeyR<R, X> = <X as PlaceKey<R>>::Key;

/// The positions of one component: its own needs', its build needs' and
/// its subscriptions'.
#[allow(clippy::type_complexity)]
pub struct CompPositions<In, Ib, Is, Ir>(PhantomData<fn() -> (In, Ib, Is, Ir)>);

/// One component `C` of the part `O` proved at the path type `X` of `Q`.
#[diagnostic::on_unimplemented(
    message = "the hooks of `{Self}` cannot be lent at `{X}` under `{Q}`",
    label = "a need of this component is not met",
    note = "a need in the component's `Needs` (lent with the component itself at its node) or `BuildNeeds` (lent at the parent; none at the root) is not on this structure or overlaps another, a subscription's anchor or steps are not there, or what its hooks return is not a request of this structure"
)]
pub trait ProveComp<Q, X, O, I> {
    /// The component's positions.
    type Pos;
}
type P_<Q> = PartsOf<Q>;
impl<Q, X, O, C, In, Ib, Is, Ir> ProveComp<Q, X, O, (In, Ib, Is, Ir)> for C
where
    Q: Composition<Parts: SelfParts, Root: PlacesUnder<End, Out: PlaceRequests<RootOf<Q>>>>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil>,
    C: Hooks<O, P_<Q>, TargetOf<Q, X>>,
    OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>>:
        LendAt<'static, RootOf<Q>, P_<Q>, X, In, LendInstant<Q>> + Writes<P_<Q>>,
    X: BuildAt<'static, RootOf<Q>, P_<Q>, C::BuildNeeds, Ib, LendInstant<Q>>,
    C::BuildNeeds: Writes<P_<Q>>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, P_<Q>, Is>,
    C::Returns: IntoRequestSum<RequestSumOf<RootOf<Q>>, Ir>,
{
    type Pos = CompPositions<In, Ib, Is, Ir>;
}

/// Lending the hooks of the component `C` of the part `O` at the path
/// type `X` of `Q`, for every lifetime of the lend.
pub trait CompLend<'a, Q, X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>>, O, C>
where
    Q: Composition<Parts: SelfParts, Root: PlacesUnder<End, Out: PlaceRequests<RootOf<Q>>>>,
    C: Hooks<O, P_<Q>, TargetOf<Q, X>>,
{
    /// The keyed positions of the hooks' view.
    type Positions: HookKeys<C, O, P_<Q>, TargetOf<Q, X>>;
    /// The keyed positions of `build`'s view.
    type BuildPositions: KeyedList<P_<Q>, BuildKeyed<C, O, P_<Q>, TargetOf<Q, X>>>;
    /// Lend the node at `at` to a hook.
    #[allow(clippy::type_complexity)]
    fn lend<T>(
        root: &'a mut RootNode<Q>,
        at: X,
        outbox: &'a OutboxOf<Q>,
        f: impl FnOnce(HookView<'a, C, O, P_<Q>, TargetOf<Q, X>, Self::Positions>) -> T,
    ) -> Result<T, LendError>;
    /// Lend the parent at `parent` to `build`, the new node's key `key`.
    #[allow(clippy::type_complexity)]
    fn lend_parent<T>(
        root: Option<&'a mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &'a PlaceKeyOf<Q, X>,
        outbox: &'a OutboxOf<Q>,
        f: impl FnOnce(BuildView<'a, C, O, P_<Q>, TargetOf<Q, X>, Self::BuildPositions>) -> T,
    ) -> Result<T, LendError>;
    /// What the component subscribes to, resolved at the place.
    fn subscriptions(out: &mut Vec<Subscription>);
    /// The components its hooks take exclusive.
    fn writes(out: &mut Vec<TypeId>);
    /// The components its `build` takes exclusive at the parent.
    fn build_writes(out: &mut Vec<TypeId>);
    /// What a hook returned, in the requests of the tree.
    fn returned(r: C::Returns) -> RequestSumOf<RootOf<Q>>;
}
impl<'a, Q, X, O, C, In, Ib, Is, Ir> CompLend<'a, Q, X, O, C> for CompPositions<In, Ib, Is, Ir>
where
    Q: Composition<Parts: SelfParts, Root: PlacesUnder<End, Out: PlaceRequests<RootOf<Q>>>>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil> + ParentPath,
    C: Hooks<O, P_<Q>, TargetOf<Q, X>>,
    OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>>:
        LendAt<'a, RootOf<Q>, P_<Q>, X, In, LendInstant<Q>> + Writes<P_<Q>>,
    X: BuildAt<'a, RootOf<Q>, P_<Q>, C::BuildNeeds, Ib, LendInstant<Q>>,
    C::BuildNeeds: Writes<P_<Q>>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, P_<Q>, Is>,
    C::Returns: IntoRequestSum<RequestSumOf<RootOf<Q>>, Ir>,
{
    type Positions = <OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>> as LendAt<
        'a,
        RootOf<Q>,
        P_<Q>,
        X,
        In,
        LendInstant<Q>,
    >>::Positions;
    type BuildPositions =
        <X as BuildAt<'a, RootOf<Q>, P_<Q>, C::BuildNeeds, Ib, LendInstant<Q>>>::Positions;
    fn lend<T>(
        root: &'a mut RootNode<Q>,
        at: X,
        outbox: &'a OutboxOf<Q>,
        f: impl FnOnce(HookView<'a, C, O, P_<Q>, TargetOf<Q, X>, Self::Positions>) -> T,
    ) -> Result<T, LendError> {
        <OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>> as LendAt<
            'a,
            RootOf<Q>,
            P_<Q>,
            X,
            In,
            LendInstant<Q>,
        >>::lend(root, at, outbox, FromSplit, f)
    }
    fn lend_parent<T>(
        root: Option<&'a mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &'a PlaceKeyOf<Q, X>,
        outbox: &'a OutboxOf<Q>,
        f: impl FnOnce(BuildView<'a, C, O, P_<Q>, TargetOf<Q, X>, Self::BuildPositions>) -> T,
    ) -> Result<T, LendError> {
        <X as BuildAt<'a, RootOf<Q>, P_<Q>, C::BuildNeeds, Ib, LendInstant<Q>>>::lend(
            root, parent, key, outbox, f,
        )
    }
    fn subscriptions(out: &mut Vec<Subscription>) {
        <C::Subscribes as Subscriptions<ChainAt<Q, X>, P_<Q>, Is>>::subscriptions(out);
    }
    fn writes(out: &mut Vec<TypeId>) {
        <OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>> as Writes<P_<Q>>>::writes(out);
    }
    fn build_writes(out: &mut Vec<TypeId>) {
        <C::BuildNeeds as Writes<P_<Q>>>::writes(out);
    }
    fn returned(r: C::Returns) -> RequestSumOf<RootOf<Q>> {
        r.into_sum()
    }
}

// ---------------------------------------------------------------- proving a place

/// Every component of a share `Self` of the part `O` proved at `X`.
pub trait ProveShare<Q, X, O, I> {
    /// The positions, one per component.
    type Out;
}
impl<Q, X, O> ProveShare<Q, X, O, HNil> for HNil {
    type Out = HNil;
}
impl<Q, X, O, C: ProveComp<Q, X, O, IC>, T: ProveShare<Q, X, O, IT>, IC, IT>
    ProveShare<Q, X, O, HCons<IC, IT>> for HCons<C, T>
{
    type Out = HCons<C::Pos, T::Out>;
}
/// Every share of the parts `Self` proved at `X`.
pub trait ProveParts<Q, X, I> {
    /// The positions, one list per part.
    type Out;
}
impl<Q, X> ProveParts<Q, X, HNil> for HNil {
    type Out = HNil;
}
impl<Q, X, O, Ps, IO, IT> ProveParts<Q, X, HCons<IO, IT>> for HCons<O, Ps>
where
    Q: Composition,
    X: PathFrom<RootOf<Q>>,
    O: Share<TargetOf<Q, X>>,
    O::Data: ProveShare<Q, X, O, IO>,
    Ps: ProveParts<Q, X, IT>,
{
    type Out = HCons<<O::Data as ProveShare<Q, X, O, IO>>::Out, Ps::Out>;
}
/// The positions of a place: of every component (`PL`), the places of its
/// children (`Ix`, a list of [`ChildPlace`]), and the place its nodes link
/// to (`Lk`, [`LinkPlace`] or [`NoLinkPlace`]).
#[allow(clippy::type_complexity)]
pub struct PlacePositions<PL, Ix, Lk>(PhantomData<fn() -> (PL, Ix, Lk)>);
/// The place `Xc` of a child, its positions `Pc`, at the number `N` among
/// the places of the tree.
#[allow(clippy::type_complexity)]
pub struct ChildPlace<Xc, Pc, N>(PhantomData<fn() -> (Xc, Pc, N)>);
/// The place `Xb` the nodes of a place link to, its positions `Pb`, at the
/// number `N`.
#[allow(clippy::type_complexity)]
pub struct LinkPlace<Xb, Pb, N>(PhantomData<fn() -> (Xb, Pb, N)>);
/// The nodes of a place link to none.
pub struct NoLinkPlace;

/// The numbers of the places of the children of a node at `X` of `Q`'s
/// tree, from the child at `Pos` on; `I` is inferred.
pub trait ProveChildPlaces<Q, X, Pos, I> {
    /// A list of [`ChildPlace`].
    type Out;
}
impl<Q, X, Pos> ProveChildPlaces<Q, X, Pos, HNil> for HNil {
    type Out = HNil;
}
type AppendOf<X, Pos, H> = <X as Append<Pos, H>>::Out;
impl<Q, X, Pos, CK, S, T, N, IC, IT> ProveChildPlaces<Q, X, Pos, HCons<(N, IC), IT>>
    for HCons<Child<CK, S>, T>
where
    Q: Composition<Root: PlacesUnder<End>>,
    S: ChildStorage,
    X: Append<Pos, S::Handle>,
    PlacesOf<Q>: FindPlace<AppendOf<X, Pos, S::Handle>, N>,
    Place<AppendOf<X, Pos, S::Handle>>: ProvePlace<Q, IC>,
    T: ProveChildPlaces<Q, X, There<Pos>, IT>,
{
    type Out = HCons<
        ChildPlace<
            AppendOf<X, Pos, S::Handle>,
            <Place<AppendOf<X, Pos, S::Handle>> as ProvePlace<Q, IC>>::Pos,
            N,
        >,
        T::Out,
    >;
}
/// The number of the place a node's links (`Self`, its kind's
/// [`NodeKind::Linked`]) point to, in `Q`'s tree; `I` is inferred.
pub trait ProveLinkPlace<Q, I> {
    /// [`LinkPlace`] or [`NoLinkPlace`].
    type Out;
}
impl<Q> ProveLinkPlace<Q, ()> for HNil {
    type Out = NoLinkPlace;
}
type RootChildAt<Q, L> =
    Step<L, crate::structure::HandleAt<<RootOf<Q> as NodeKind>::Children, L>, End>;
impl<Q, B, L, N, IB> ProveLinkPlace<Q, (L, N, IB)> for HCons<crate::structure::ByOwnKey<B>, HNil>
where
    Q: Composition<Root: PlacesUnder<End>>,
    B: NodeKind,
    <RootOf<Q> as NodeKind>::Children: crate::structure::ChildOfKind<B, L>,
    PlacesOf<Q>: FindPlace<RootChildAt<Q, L>, N>,
    Place<RootChildAt<Q, L>>: ProvePlace<Q, IB>,
{
    type Out =
        LinkPlace<RootChildAt<Q, L>, <Place<RootChildAt<Q, L>> as ProvePlace<Q, IB>>::Pos, N>;
}

/// One place proved.
pub trait ProvePlace<Q, I> {
    /// Its positions.
    type Pos;
}
impl<Q: Composition, X, Ip, Ic, Il> ProvePlace<Q, (Ip, Ic, Il)> for Place<X>
where
    Q::Root: PlacesUnder<End>,
    X: PathFrom<RootOf<Q>>,
    PartsOf<Q>: ProveParts<Q, X, Ip>,
    <TargetOf<Q, X> as NodeKind>::Children: ProveChildPlaces<Q, X, I0, Ic>,
    <TargetOf<Q, X> as NodeKind>::Linked: ProveLinkPlace<Q, Il>,
{
    type Pos = PlacePositions<
        <PartsOf<Q> as ProveParts<Q, X, Ip>>::Out,
        <<TargetOf<Q, X> as NodeKind>::Children as ProveChildPlaces<Q, X, I0, Ic>>::Out,
        <<TargetOf<Q, X> as NodeKind>::Linked as ProveLinkPlace<Q, Il>>::Out,
    >;
}
/// Every place of a list proved (`I`, one variable for the tree); the
/// pack is the list of their positions.
pub trait ProvePlaces<Q, I> {
    /// The pack.
    type Out;
}
impl<Q> ProvePlaces<Q, HNil> for HNil {
    type Out = HNil;
}
impl<Q, S: ProvePlace<Q, IS>, T: ProvePlaces<Q, IT>, IS, IT> ProvePlaces<Q, HCons<IS, IT>>
    for HCons<S, T>
{
    type Out = HCons<S::Pos, T::Out>;
}
/// The pack of the places `L` of `Q`; [`composition!`](crate::composition)
/// defines the opaque pack with it.
pub fn prove_places<Q, L: ProvePlaces<Q, I>, I>() -> PhantomData<L::Out> {
    PhantomData
}

// ---------------------------------------------------------------- lends of hooks

/// What a hook's lend came to. The driver lends a hook only at a node it
/// has, so the lend finds it: debug builds check it.
fn lent<T>(r: Result<T, LendError>) -> Option<T> {
    debug_assert!(
        r.is_ok(),
        "a hook is lent at a node the driver has, so the lend finds it"
    );
    r.ok()
}
trait Lent<T> {
    fn lent(self) -> Option<T>;
}
impl<T> Lent<T> for Result<T, LendError> {
    fn lent(self) -> Option<T> {
        lent(self)
    }
}

// ---------------------------------------------------------------- what the driver keeps of a place

/// A component of a place, as the driver keeps it.
#[derive(Debug)]
pub struct CompInfo {
    /// Its type.
    pub type_id: TypeId,
    /// Whether it has a deadline.
    pub timed: bool,
    /// Whether it acts before the calls.
    pub acts_before_calls: bool,
    /// What it subscribes to.
    pub subscriptions: Vec<Subscription>,
    /// The components its hooks take exclusive.
    pub writes: Vec<TypeId>,
    /// The components its `build` takes exclusive at the parent.
    pub build_writes: Vec<TypeId>,
}
/// A place, as the driver keeps it.
#[derive(Debug)]
pub struct PlaceInfo {
    /// Its path type.
    pub path: TypeId,
    /// The parent's path type; none for the root.
    pub parent: Option<TypeId>,
    /// Its position among the children of its parent's kind; none for the
    /// root.
    pub index: Option<usize>,
    /// Its kind.
    pub kind: TypeId,
    /// The kinds its kind links to ([`NodeKind::Linked`]).
    pub links: Vec<TypeId>,
    /// Its positions up to the root, as subscriptions name places.
    pub pre: TypeId,
    /// How deep its nodes are; the root at 0.
    pub depth: usize,
    /// [`NodeStarted`] of its kind.
    pub started: TypeId,
    /// [`NodeEnding`] of its kind.
    pub ending: TypeId,
    /// Its components, part by part.
    pub comps: Vec<CompInfo>,
}

// ---------------------------------------------------------------- a share's hooks

/// The hooks of the components `Cs` of the share of the part `O` at `X`,
/// one by one; `Self` is their positions. An index `c` counts the share's
/// components; a hook at an index past them gives `None`.
#[allow(clippy::type_complexity, missing_docs)]
pub trait ShareOps<
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>>,
    O,
    Cs,
>
{
    /// How many components.
    const LEN: usize;
    fn info(out: &mut Vec<CompInfo>);
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &PlaceKeyOf<Q, X>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, X>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<Cs, LendError>;
    fn deadline(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        o: &OutboxOf<Q>,
    ) -> Option<Option<InstantOf<Q>>>;
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>>;
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>>;
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>>;
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &OutboxOf<Q>);
    fn end(
        root: &mut RootNode<Q>,
        at: X,
        cause: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    );
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut <O as Share<TargetOf<Q, X>>>::Remains,
        o: &OutboxOf<Q>,
    ) where
        O: Share<TargetOf<Q, X>>;
}
impl<Q: DriverComposition, X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>>, O>
    ShareOps<Q, X, O, HNil> for HNil
{
    const LEN: usize = 0;
    fn info(_: &mut Vec<CompInfo>) {}
    fn build(
        _: &mut Option<&mut RootNode<Q>>,
        _: &ParentOf<X>,
        _: &PlaceKeyOf<Q, X>,
        _: &ArgsOf<P_<Q>>,
        _: &mut StartOf<TargetOf<Q, X>>,
        _: &OutboxOf<Q>,
    ) -> Result<HNil, LendError> {
        Ok(HNil)
    }
    fn deadline(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: &OutboxOf<Q>,
    ) -> Option<Option<InstantOf<Q>>> {
        None
    }
    fn expire(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: InstantOf<Q>,
        _: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        None
    }
    fn on_events(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: Triggered<'_>,
        _: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        None
    }
    fn before_calls(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: &Waker,
        _: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        None
    }
    fn aborted(_: &mut RootNode<Q>, _: X, _: AbortCause, _: &OutboxOf<Q>) {}
    fn end(
        _: &mut RootNode<Q>,
        _: X,
        _: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        _: &OutboxOf<Q>,
    ) {
    }
    fn remains(
        _: &mut RootNode<Q>,
        _: X,
        _: &mut <O as Share<TargetOf<Q, X>>>::Remains,
        _: &OutboxOf<Q>,
    ) where
        O: Share<TargetOf<Q, X>>,
    {
    }
}
impl<Q, X, O, C, Cs, CP, SP> ShareOps<Q, X, O, HCons<C, Cs>> for HCons<CP, SP>
where
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>> + Copy,
    C: Hooks<O, P_<Q>, TargetOf<Q, X>>,
    C::Timing: Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>,
    CP: for<'a> CompLend<'a, Q, X, O, C>,
    SP: ShareOps<Q, X, O, Cs>,
{
    const LEN: usize = 1 + SP::LEN;
    fn info(out: &mut Vec<CompInfo>) {
        let mut subscriptions = Vec::new();
        <CP as CompLend<'_, Q, X, O, C>>::subscriptions(&mut subscriptions);
        let mut writes = Vec::new();
        <CP as CompLend<'_, Q, X, O, C>>::writes(&mut writes);
        let mut build_writes = Vec::new();
        <CP as CompLend<'_, Q, X, O, C>>::build_writes(&mut build_writes);
        out.push(CompInfo {
            build_writes,
            type_id: TypeId::of::<C>(),
            timed: <C::Timing as Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>>::TIMED,
            acts_before_calls: C::ACTS_BEFORE_CALLS,
            subscriptions,
            writes,
        });
        SP::info(out);
    }
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &PlaceKeyOf<Q, X>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, X>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<HCons<C, Cs>, LendError> {
        let head = <CP as CompLend<'_, Q, X, O, C>>::lend_parent(
            root.as_deref_mut(),
            parent,
            key,
            outbox,
            |view| C::build::<<CP as CompLend<'_, Q, X, O, C>>::BuildPositions>(view, args, start),
        )?;
        Ok(HCons {
            head,
            tail: SP::build(root, parent, key, args, start, outbox)?,
        })
    }
    fn deadline(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        o: &OutboxOf<Q>,
    ) -> Option<Option<InstantOf<Q>>> {
        if c > 0 {
            return SP::deadline(root, at, c - 1, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| {
                <C::Timing as Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>>::deadline(&view)
            })
            .lent()
            .flatten(),
        )
    }
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        if c > 0 {
            return SP::expire(root, at, c - 1, now, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| {
                <C::Timing as Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>>::expire(view, now)
            })
            .lent()
            .flatten()
            .map(<CP as CompLend<'_, Q, X, O, C>>::returned),
        )
    }
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        if c > 0 {
            return SP::on_events(root, at, c - 1, events, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| C::on_events(events, view))
                .lent()
                .flatten()
                .map(<CP as CompLend<'_, Q, X, O, C>>::returned),
        )
    }
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        if c > 0 {
            return SP::before_calls(root, at, c - 1, waker, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| {
                C::before_calls(view, waker)
            })
            .lent()
            .flatten()
            .map(<CP as CompLend<'_, Q, X, O, C>>::returned),
        )
    }
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &OutboxOf<Q>) {
        lent(<CP as CompLend<'_, Q, X, O, C>>::lend(
            root,
            at,
            o,
            |view| C::aborted(view, cause),
        ));
        SP::aborted(root, at, cause, o);
    }
    fn end(
        root: &mut RootNode<Q>,
        at: X,
        cause: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    ) {
        lent(<CP as CompLend<'_, Q, X, O, C>>::lend(
            root,
            at,
            o,
            |view| C::end(view, cause),
        ));
        SP::end(root, at, cause, o);
    }
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut <O as Share<TargetOf<Q, X>>>::Remains,
        o: &OutboxOf<Q>,
    ) where
        O: Share<TargetOf<Q, X>>,
    {
        lent(<CP as CompLend<'_, Q, X, O, C>>::lend(
            root,
            at,
            o,
            |view| C::remains(view, out),
        ));
        SP::remains(root, at, out, o);
    }
}

// ---------------------------------------------------------------- a node's hooks

/// The hooks of every share of the parts `Ps` at `X`, part by part;
/// `Self` is their positions. An index `c` counts the components of the
/// place, part by part.
#[allow(clippy::type_complexity, missing_docs)]
pub trait PartsOps<
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>>,
    Ps,
> where
    Ps: Shares<TargetOf<Q, X>> + RemainsList<TargetOf<Q, X>>,
{
    fn info(out: &mut Vec<CompInfo>);
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &PlaceKeyOf<Q, X>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, X>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<<Ps as Shares<TargetOf<Q, X>>>::Data, LendError>;
    fn deadline(root: &mut RootNode<Q>, at: X, c: usize, o: &OutboxOf<Q>) -> Option<InstantOf<Q>>;
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>>;
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>>;
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>>;
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &OutboxOf<Q>);
    fn end(
        root: &mut RootNode<Q>,
        at: X,
        cause: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    );
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut RemainsOf<Ps, TargetOf<Q, X>>,
        o: &OutboxOf<Q>,
    );
}
impl<Q: DriverComposition, X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>>>
    PartsOps<Q, X, HNil> for HNil
{
    fn info(_: &mut Vec<CompInfo>) {}
    fn build(
        _: &mut Option<&mut RootNode<Q>>,
        _: &ParentOf<X>,
        _: &PlaceKeyOf<Q, X>,
        _: &ArgsOf<P_<Q>>,
        _: &mut StartOf<TargetOf<Q, X>>,
        _: &OutboxOf<Q>,
    ) -> Result<HNil, LendError> {
        Ok(HNil)
    }
    fn deadline(_: &mut RootNode<Q>, _: X, _: usize, _: &OutboxOf<Q>) -> Option<InstantOf<Q>> {
        None
    }
    fn expire(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: InstantOf<Q>,
        _: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn on_events(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: Triggered<'_>,
        _: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn before_calls(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: &Waker,
        _: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn aborted(_: &mut RootNode<Q>, _: X, _: AbortCause, _: &OutboxOf<Q>) {}
    fn end(
        _: &mut RootNode<Q>,
        _: X,
        _: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        _: &OutboxOf<Q>,
    ) {
    }
    fn remains(_: &mut RootNode<Q>, _: X, _: &mut HNil, _: &OutboxOf<Q>) {}
}
impl<Q, X, O, Ps, SP, PL> PartsOps<Q, X, HCons<O, Ps>> for HCons<SP, PL>
where
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>> + Copy,
    O: Share<TargetOf<Q, X>> + 'static,
    Ps: Shares<TargetOf<Q, X>> + RemainsList<TargetOf<Q, X>>,
    SP: ShareOps<Q, X, O, O::Data>,
    PL: PartsOps<Q, X, Ps>,
{
    fn info(out: &mut Vec<CompInfo>) {
        SP::info(out);
        PL::info(out);
    }
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &PlaceKeyOf<Q, X>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, X>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<HCons<O::Data, <Ps as Shares<TargetOf<Q, X>>>::Data>, LendError> {
        Ok(HCons {
            head: SP::build(root, parent, key, args, start, outbox)?,
            tail: PL::build(root, parent, key, args, start, outbox)?,
        })
    }
    fn deadline(root: &mut RootNode<Q>, at: X, c: usize, o: &OutboxOf<Q>) -> Option<InstantOf<Q>> {
        match c.checked_sub(SP::LEN) {
            None => SP::deadline(root, at, c, o).flatten(),
            Some(c) => PL::deadline(root, at, c, o),
        }
    }
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        match c.checked_sub(SP::LEN) {
            None => SP::expire(root, at, c, now, o).flatten(),
            Some(c) => PL::expire(root, at, c, now, o),
        }
    }
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        match c.checked_sub(SP::LEN) {
            None => SP::on_events(root, at, c, events, o).flatten(),
            Some(c) => PL::on_events(root, at, c, events, o),
        }
    }
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        match c.checked_sub(SP::LEN) {
            None => SP::before_calls(root, at, c, waker, o).flatten(),
            Some(c) => PL::before_calls(root, at, c, waker, o),
        }
    }
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &OutboxOf<Q>) {
        SP::aborted(root, at, cause, o);
        PL::aborted(root, at, cause, o);
    }
    fn end(
        root: &mut RootNode<Q>,
        at: X,
        cause: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    ) {
        SP::end(root, at, cause, o);
        PL::end(root, at, cause, o);
    }
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut HCons<O::Remains, RemainsOf<Ps, TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    ) {
        SP::remains(root, at, &mut out.head, o);
        PL::remains(root, at, &mut out.tail, o);
    }
}

// ---------------------------------------------------------------- a place's hooks

/// Why a node ends when no reason of its kind's is given: the causes of
/// [`EndCause`] but its reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndBy {
    /// A node above it ended.
    Above,
    /// The last node linking to it ended.
    Unlinked,
    /// The driver was dropped.
    Dropped,
}
impl EndBy {
    /// The cause for a kind whose reason is `R`.
    pub fn cause<'a, R>(self) -> EndCause<'a, R> {
        match self {
            Self::Above => EndCause::Above,
            Self::Unlinked => EndCause::Unlinked,
            Self::Dropped => EndCause::Dropped,
        }
    }
}

/// Taking the node at a path out of its parent's store; the root is not
/// taken out this way.
pub trait Detach<R: KindFor<P>, P> {
    /// Take it out; `false` when it was not there.
    fn detach(self, root: &mut Node<R, P>) -> bool;
}
impl<R: KindFor<P>, P> Detach<R, P> for End {
    fn detach(self, _: &mut Node<R, P>) -> bool {
        false
    }
}
impl<R: KindFor<P>, P, I, H: Copy, Rr> Detach<R, P> for Step<I, H, Rr>
where
    Step<I, H, Rr>: SplitLast,
    <Step<I, H, Rr> as SplitLast>::Parent: NodePath<R, P>,
    LastChildren<R, Step<I, H, Rr>>: ChildAt<
            <Step<I, H, Rr> as SplitLast>::Index,
            P,
            Storage: ChildStorage<Handle = <Step<I, H, Rr> as SplitLast>::Handle>,
        >,
{
    fn detach(self, root: &mut Node<R, P>) -> bool {
        remove(root, self).is_some()
    }
}

/// The type of a path's parent place, as the driver keeps it.
pub trait ParentPlace {
    /// The parent's path type; none for the root.
    fn parent_place() -> Option<TypeId>;
    /// The position of the path's last step among the children of the
    /// parent's kind; none for the root.
    fn child_index() -> Option<usize>;
}
impl ParentPlace for End {
    fn parent_place() -> Option<TypeId> {
        None
    }
    fn child_index() -> Option<usize> {
        None
    }
}
impl<I, H, R> ParentPlace for Step<I, H, R>
where
    Step<I, H, R>: ParentPath<Parent: 'static> + SplitLast<Index: super::walk::Number>,
{
    fn parent_place() -> Option<TypeId> {
        Some(TypeId::of::<ParentOf<Self>>())
    }
    fn child_index() -> Option<usize> {
        Some(<<Self as SplitLast>::Index as super::walk::Number>::VALUE)
    }
}

/// The nodes at a path, reached by the driver's addresses: added under a
/// parent by a key, found by their key, moved under another parent. The
/// root is none of these.
#[allow(clippy::type_complexity)]
pub trait ChildPath<R: KindFor<P>, P>: PathFrom<R> + ParentPath + PlaceKey<R> {
    /// The parent's path, from its address.
    fn parent_path(parent: &Address<R>) -> Option<ParentOf<Self>>;
    /// Keep `node` under the parent at `parent`, under `key` (a key of
    /// this path's store); `None` when the key is taken.
    fn attach(
        root: &mut Node<R, P>,
        parent: &Address<R>,
        key: <Self as PlaceKey<R>>::Key,
        node: Node<Self::Target, P>,
    ) -> Option<Address<R>>
    where
        Self::Target: KindFor<P>;
    /// The node kept under `key` under the parent at `parent`.
    fn find_under(
        root: &Node<R, P>,
        parent: &Address<R>,
        key: &<Self as PlaceKey<R>>::Key,
    ) -> Option<Address<R>>;
    /// Move the node at `at` under the node at `to`, of its parent's place.
    fn move_under(
        root: &mut Node<R, P>,
        at: &Address<R>,
        to: &Address<R>,
    ) -> Result<Address<R>, MoveError>;
}
impl<R: KindFor<P>, P> ChildPath<R, P> for End {
    fn parent_path(_: &Address<R>) -> Option<()> {
        None
    }
    fn attach(_: &mut Node<R, P>, _: &Address<R>, _: (), _: Node<R, P>) -> Option<Address<R>>
    where
        R: KindFor<P>,
    {
        None
    }
    fn find_under(_: &Node<R, P>, _: &Address<R>, _: &()) -> Option<Address<R>> {
        None
    }
    fn move_under(
        _: &mut Node<R, P>,
        _: &Address<R>,
        _: &Address<R>,
    ) -> Result<Address<R>, MoveError> {
        Err(MoveError::NoSource)
    }
}
type ParentPathOf<X> = <X as SplitLast>::Parent;
type IndexOf<X> = <X as SplitLast>::Index;
type HandleOf<X> = <X as SplitLast>::Handle;
impl<R, P, I, H, Rr> ChildPath<R, P> for Step<I, H, Rr>
where
    R: KindFor<P>,
    Self: PlaceKey<R, Key = KeyAt<LastChildren<R, Self>, IndexOf<Self>>>,
    Self: PathFrom<R> + FromAddress<R> + SplitLast + ParentPath<Parent = ParentPathOf<Self>>,
    ParentPathOf<Self>:
        NodePath<R, P> + FromAddress<R> + Append<IndexOf<Self>, HandleOf<Self>, Out = Self> + Copy,
    LastChildren<R, Self>: ChildAt<
            IndexOf<Self>,
            P,
            Kind = <Self as PathFrom<R>>::Target,
            Storage: ChildStorage<Handle = HandleOf<Self>>,
        >,
{
    fn parent_path(parent: &Address<R>) -> Option<ParentPathOf<Self>> {
        ParentPathOf::<Self>::from_address(parent)
    }
    fn attach(
        root: &mut Node<R, P>,
        parent: &Address<R>,
        key: <Self as PlaceKey<R>>::Key,
        node: Node<Self::Target, P>,
    ) -> Option<Address<R>>
    where
        Self::Target: KindFor<P>,
    {
        let pp = ParentPathOf::<Self>::from_address(parent)?;
        let parent = pp.node_mut(root)?;
        let store =
            <LastChildren<R, Self> as ChildAt<IndexOf<Self>, P>>::store_mut(&mut parent.children);
        let h = store.insert(key, node).ok()?;
        Some(pp.append(h).address())
    }
    fn find_under(
        root: &Node<R, P>,
        parent: &Address<R>,
        key: &<Self as PlaceKey<R>>::Key,
    ) -> Option<Address<R>> {
        let pp = ParentPathOf::<Self>::from_address(parent)?;
        let parent = pp.node(root)?;
        let store = <LastChildren<R, Self> as ChildAt<IndexOf<Self>, P>>::store(&parent.children);
        Some(pp.append(store.find(key)?).address())
    }
    fn move_under(
        root: &mut Node<R, P>,
        at: &Address<R>,
        to: &Address<R>,
    ) -> Result<Address<R>, MoveError> {
        let at = Self::from_address(at).ok_or(MoveError::NoSource)?;
        let to = ParentPathOf::<Self>::from_address(to).ok_or(MoveError::NoDestination)?;
        Ok(move_to(root, at, to)?.address())
    }
}

/// The hooks of every component of the place `Pl` of `Q`, reached by the
/// address of a node there; `Self` is the place's positions. A node not
/// there is skipped.
#[allow(clippy::type_complexity, missing_docs)]
pub trait PlaceOps<Q: DriverComposition, Pl: PlaceDesc<RootOf<Q>>> {
    /// The places of its children ([`ChildPlace`]s): the edit handle's
    /// operations on each.
    type Children: crate::request::sealed::ChildEdits<Q, <TargetOf<Q, Pl::At> as NodeKind>::Children>;
    /// The place its nodes link to: building the linked node.
    type Link: super::edit::LinkAdd<Q, TargetOf<Q, Pl::At>, PlaceKeyOf<Q, Pl::At>>;
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
    Lk: super::edit::LinkAdd<Q, TargetOf<Q, X>, PlaceKeyOf<Q, X>>,
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
    type Children = super::edit::NoPlaceEdits;
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

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
    call::{ChainAt, Composition, PartsOf, RootOf},
    event::{NodeEnding, NodeStarted, Outbox, OwnPlace, Subscription, Subscriptions, Triggered},
    hooks::{
        AbortCause, ArgsOf, BuildKeyed, BuildView, EndReasonOf, HookKeys, HookView, Hooks,
        OwnNeeds, PartList, Timing,
    },
    lend::{Flat, KeyedList, LendAt, LendError, NeedSet, ViewOf, Writes},
    need::ChainOf,
    request::{IntoRequestSum, PlaceRequests, RequestSumOf},
    structure::{
        Address, Append, Child, ChildAt, ChildStorage, End, FromAddress, KindFor, LastChildren,
        MoveError, Node, NodeKind, NodePath, ParentPath, PathFrom, RemainsList, RemainsOf, Share,
        Shares, SplitLast, Step, Store, move_to, remove,
    },
};

// ---------------------------------------------------------------- casts

/// `a` as a `B`, when `A` is `B`. The driver reaches a place by its number
/// and hands it a key or a reason of the type the edit handle's types
/// give; the place's own types are the same, which only the run time sees.
fn cast<A: 'static, B: 'static>(a: A) -> Option<B> {
    let mut a = Some(a);
    (&mut a as &mut dyn core::any::Any)
        .downcast_mut::<Option<B>>()
        .and_then(Option::take)
}
/// `a` as a `&B`, when `A` is `B`.
fn cast_ref<A: 'static, B: 'static>(a: &A) -> Option<&B> {
    (a as &dyn core::any::Any).downcast_ref::<B>()
}
/// The edit handle's types are the place's: the driver found the place by
/// the position of the child the handle's types name.
const SAME_TYPES: &str = "the place found by the position of a child has the types of that child";

// ---------------------------------------------------------------- the places

/// The nodes a path type `X` from the root leads to.
pub struct Place<X>(PhantomData<fn() -> X>);
/// The end of a list of places, a filler the driver never reaches.
pub struct NoPlace;

/// What a place of a tree whose root is of the kind `R` is: its path
/// type.
pub trait PlaceDesc<R: NodeKind> {
    /// The path type.
    type At: ParentPath + PathFrom<R> + 'static;
}
impl<R: NodeKind, X: ParentPath + PathFrom<R> + 'static> PlaceDesc<R> for Place<X> {
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
    type Clock: MonotonicClock + 'static;
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
/// `Self`, for the needs `N`; at the root there is no parent and `N` is
/// empty.
pub trait BuildAt<'a, R: KindFor<P>, P, N: NeedSet<P>, I>: ParentPath {
    /// The keyed positions.
    type Positions: KeyedList<P, <N as Flat>::Keyed>;
    /// Lend the parent at `parent` to `f`.
    fn lend<T>(
        root: Option<&'a mut Node<R, P>>,
        parent: &Self::Parent,
        outbox: &'a Outbox,
        f: impl FnOnce(ViewOf<'a, N, P, Self::Positions>) -> T,
    ) -> Result<T, LendError>;
}
impl<'a, R: KindFor<P>, P> BuildAt<'a, R, P, HNil, ()> for End {
    type Positions = HNil;
    fn lend<T>(
        _: Option<&'a mut Node<R, P>>,
        _: &(),
        _: &'a Outbox,
        f: impl FnOnce(HNil) -> T,
    ) -> Result<T, LendError> {
        Ok(f(HNil))
    }
}
impl<'a, R: KindFor<P>, P, N, I, J, H, Rr> BuildAt<'a, R, P, N, I> for Step<J, H, Rr>
where
    Step<J, H, Rr>: ParentPath,
    N: LendAt<'a, R, P, ParentOf<Step<J, H, Rr>>, I>,
{
    type Positions = N::Positions;
    fn lend<T>(
        root: Option<&'a mut Node<R, P>>,
        parent: &ParentOf<Self>,
        outbox: &'a Outbox,
        f: impl FnOnce(ViewOf<'a, N, P, Self::Positions>) -> T,
    ) -> Result<T, LendError> {
        N::lend(root.ok_or(LendError::NoNode)?, *parent, outbox, f)
    }
}

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
    OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>>: LendAt<'static, RootOf<Q>, P_<Q>, X, In> + Writes<P_<Q>>,
    X: BuildAt<'static, RootOf<Q>, P_<Q>, C::BuildNeeds, Ib>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, P_<Q>, Is>,
    C::Returns: IntoRequestSum<RequestSumOf<RootOf<Q>>, Ir>,
{
    type Pos = CompPositions<In, Ib, Is, Ir>;
}

/// Lending the hooks of the component `C` of the part `O` at the path
/// type `X` of `Q`, for every lifetime of the lend.
pub trait CompLend<'a, Q, X: PathFrom<RootOf<Q>> + ParentPath, O, C>
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
        outbox: &'a Outbox,
        f: impl FnOnce(HookView<'a, C, O, P_<Q>, TargetOf<Q, X>, Self::Positions>) -> T,
    ) -> Result<T, LendError>;
    /// Lend the parent at `parent` to `build`.
    #[allow(clippy::type_complexity)]
    fn lend_parent<T>(
        root: Option<&'a mut RootNode<Q>>,
        parent: &ParentOf<X>,
        outbox: &'a Outbox,
        f: impl FnOnce(BuildView<'a, C, O, P_<Q>, TargetOf<Q, X>, Self::BuildPositions>) -> T,
    ) -> Result<T, LendError>;
    /// What the component subscribes to, resolved at the place.
    fn subscriptions(out: &mut Vec<Subscription>);
    /// The components its hooks take exclusive.
    fn writes(out: &mut Vec<TypeId>);
    /// What a hook returned, in the requests of the tree.
    fn returned(r: C::Returns) -> RequestSumOf<RootOf<Q>>;
}
impl<'a, Q, X, O, C, In, Ib, Is, Ir> CompLend<'a, Q, X, O, C> for CompPositions<In, Ib, Is, Ir>
where
    Q: Composition<Parts: SelfParts, Root: PlacesUnder<End, Out: PlaceRequests<RootOf<Q>>>>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil> + ParentPath,
    C: Hooks<O, P_<Q>, TargetOf<Q, X>>,
    OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>>: LendAt<'a, RootOf<Q>, P_<Q>, X, In> + Writes<P_<Q>>,
    X: BuildAt<'a, RootOf<Q>, P_<Q>, C::BuildNeeds, Ib>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, P_<Q>, Is>,
    C::Returns: IntoRequestSum<RequestSumOf<RootOf<Q>>, Ir>,
{
    type Positions =
        <OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>> as LendAt<'a, RootOf<Q>, P_<Q>, X, In>>::Positions;
    type BuildPositions = <X as BuildAt<'a, RootOf<Q>, P_<Q>, C::BuildNeeds, Ib>>::Positions;
    fn lend<T>(
        root: &'a mut RootNode<Q>,
        at: X,
        outbox: &'a Outbox,
        f: impl FnOnce(HookView<'a, C, O, P_<Q>, TargetOf<Q, X>, Self::Positions>) -> T,
    ) -> Result<T, LendError> {
        <OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>> as LendAt<'a, RootOf<Q>, P_<Q>, X, In>>::lend(
            root, at, outbox, f,
        )
    }
    fn lend_parent<T>(
        root: Option<&'a mut RootNode<Q>>,
        parent: &ParentOf<X>,
        outbox: &'a Outbox,
        f: impl FnOnce(BuildView<'a, C, O, P_<Q>, TargetOf<Q, X>, Self::BuildPositions>) -> T,
    ) -> Result<T, LendError> {
        <X as BuildAt<'a, RootOf<Q>, P_<Q>, C::BuildNeeds, Ib>>::lend(root, parent, outbox, f)
    }
    fn subscriptions(out: &mut Vec<Subscription>) {
        <C::Subscribes as Subscriptions<ChainAt<Q, X>, P_<Q>, Is>>::subscriptions(out);
    }
    fn writes(out: &mut Vec<TypeId>) {
        <OwnNeeds<C, O, P_<Q>, TargetOf<Q, X>> as Writes<P_<Q>>>::writes(out);
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
/// The positions of every component of a place.
pub struct PlacePositions<PL>(PhantomData<fn() -> PL>);
/// One place proved.
pub trait ProvePlace<Q, I> {
    /// Its positions.
    type Pos;
}
impl<Q: Composition, X, I> ProvePlace<Q, I> for Place<X>
where
    PartsOf<Q>: ProveParts<Q, X, I>,
{
    type Pos = PlacePositions<<PartsOf<Q> as ProveParts<Q, X, I>>::Out>;
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
pub trait ShareOps<Q: DriverComposition, X: PathFrom<RootOf<Q>> + ParentPath, O, Cs> {
    /// How many components.
    const LEN: usize;
    fn info(out: &mut Vec<CompInfo>);
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        args: &ArgsOf<P_<Q>>,
        outbox: &Outbox,
    ) -> Result<Cs, LendError>;
    fn deadline(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        o: &Outbox,
    ) -> Option<Option<InstantOf<Q>>>;
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &Outbox,
    ) -> Option<Option<RequestsOf<Q>>>;
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &Outbox,
    ) -> Option<Option<RequestsOf<Q>>>;
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &Outbox,
    ) -> Option<Option<RequestsOf<Q>>>;
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &Outbox);
    fn end(root: &mut RootNode<Q>, at: X, reason: &EndReasonOf<TargetOf<Q, X>>, o: &Outbox);
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut <O as Share<TargetOf<Q, X>>>::Remains,
        o: &Outbox,
    ) where
        O: Share<TargetOf<Q, X>>;
}
impl<Q: DriverComposition, X: PathFrom<RootOf<Q>> + ParentPath, O> ShareOps<Q, X, O, HNil>
    for HNil
{
    const LEN: usize = 0;
    fn info(_: &mut Vec<CompInfo>) {}
    fn build(
        _: &mut Option<&mut RootNode<Q>>,
        _: &ParentOf<X>,
        _: &ArgsOf<P_<Q>>,
        _: &Outbox,
    ) -> Result<HNil, LendError> {
        Ok(HNil)
    }
    fn deadline(_: &mut RootNode<Q>, _: X, _: usize, _: &Outbox) -> Option<Option<InstantOf<Q>>> {
        None
    }
    fn expire(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: InstantOf<Q>,
        _: &Outbox,
    ) -> Option<Option<RequestsOf<Q>>> {
        None
    }
    fn on_events(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: Triggered<'_>,
        _: &Outbox,
    ) -> Option<Option<RequestsOf<Q>>> {
        None
    }
    fn before_calls(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: &Waker,
        _: &Outbox,
    ) -> Option<Option<RequestsOf<Q>>> {
        None
    }
    fn aborted(_: &mut RootNode<Q>, _: X, _: AbortCause, _: &Outbox) {}
    fn end(_: &mut RootNode<Q>, _: X, _: &EndReasonOf<TargetOf<Q, X>>, _: &Outbox) {}
    fn remains(_: &mut RootNode<Q>, _: X, _: &mut <O as Share<TargetOf<Q, X>>>::Remains, _: &Outbox)
    where
        O: Share<TargetOf<Q, X>>,
    {
    }
}
impl<Q, X, O, C, Cs, CP, SP> ShareOps<Q, X, O, HCons<C, Cs>> for HCons<CP, SP>
where
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + Copy,
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
        out.push(CompInfo {
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
        args: &ArgsOf<P_<Q>>,
        outbox: &Outbox,
    ) -> Result<HCons<C, Cs>, LendError> {
        let head = <CP as CompLend<'_, Q, X, O, C>>::lend_parent(
            root.as_deref_mut(),
            parent,
            outbox,
            |view| C::build::<<CP as CompLend<'_, Q, X, O, C>>::BuildPositions>(view, args),
        )?;
        Ok(HCons {
            head,
            tail: SP::build(root, parent, args, outbox)?,
        })
    }
    fn deadline(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        o: &Outbox,
    ) -> Option<Option<InstantOf<Q>>> {
        if c > 0 {
            return SP::deadline(root, at, c - 1, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| {
                <C::Timing as Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>>::deadline(&view)
            })
            .ok()
            .flatten(),
        )
    }
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &Outbox,
    ) -> Option<Option<RequestsOf<Q>>> {
        if c > 0 {
            return SP::expire(root, at, c - 1, now, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| {
                <C::Timing as Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>>::expire(view, now)
            })
            .ok()
            .flatten()
            .map(<CP as CompLend<'_, Q, X, O, C>>::returned),
        )
    }
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &Outbox,
    ) -> Option<Option<RequestsOf<Q>>> {
        if c > 0 {
            return SP::on_events(root, at, c - 1, events, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| C::on_events(events, view))
                .ok()
                .flatten()
                .map(<CP as CompLend<'_, Q, X, O, C>>::returned),
        )
    }
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &Outbox,
    ) -> Option<Option<RequestsOf<Q>>> {
        if c > 0 {
            return SP::before_calls(root, at, c - 1, waker, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| {
                C::before_calls(view, waker)
            })
            .ok()
            .flatten()
            .map(<CP as CompLend<'_, Q, X, O, C>>::returned),
        )
    }
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &Outbox) {
        let _ = <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| C::aborted(view, cause));
        SP::aborted(root, at, cause, o);
    }
    fn end(root: &mut RootNode<Q>, at: X, reason: &EndReasonOf<TargetOf<Q, X>>, o: &Outbox) {
        let _ = <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| C::end(view, reason));
        SP::end(root, at, reason, o);
    }
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut <O as Share<TargetOf<Q, X>>>::Remains,
        o: &Outbox,
    ) where
        O: Share<TargetOf<Q, X>>,
    {
        let _ = <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| C::remains(view, out));
        SP::remains(root, at, out, o);
    }
}

// ---------------------------------------------------------------- a node's hooks

/// The hooks of every share of the parts `Ps` at `X`, part by part;
/// `Self` is their positions. An index `c` counts the components of the
/// place, part by part.
#[allow(clippy::type_complexity, missing_docs)]
pub trait PartsOps<Q: DriverComposition, X: PathFrom<RootOf<Q>> + ParentPath, Ps>
where
    Ps: Shares<TargetOf<Q, X>> + RemainsList<TargetOf<Q, X>>,
{
    fn info(out: &mut Vec<CompInfo>);
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        args: &ArgsOf<P_<Q>>,
        outbox: &Outbox,
    ) -> Result<<Ps as Shares<TargetOf<Q, X>>>::Data, LendError>;
    fn deadline(root: &mut RootNode<Q>, at: X, c: usize, o: &Outbox) -> Option<InstantOf<Q>>;
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &Outbox,
    ) -> Option<RequestsOf<Q>>;
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &Outbox,
    ) -> Option<RequestsOf<Q>>;
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &Outbox,
    ) -> Option<RequestsOf<Q>>;
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &Outbox);
    fn end(root: &mut RootNode<Q>, at: X, reason: &EndReasonOf<TargetOf<Q, X>>, o: &Outbox);
    fn remains(root: &mut RootNode<Q>, at: X, out: &mut RemainsOf<Ps, TargetOf<Q, X>>, o: &Outbox);
}
impl<Q: DriverComposition, X: PathFrom<RootOf<Q>> + ParentPath> PartsOps<Q, X, HNil> for HNil {
    fn info(_: &mut Vec<CompInfo>) {}
    fn build(
        _: &mut Option<&mut RootNode<Q>>,
        _: &ParentOf<X>,
        _: &ArgsOf<P_<Q>>,
        _: &Outbox,
    ) -> Result<HNil, LendError> {
        Ok(HNil)
    }
    fn deadline(_: &mut RootNode<Q>, _: X, _: usize, _: &Outbox) -> Option<InstantOf<Q>> {
        None
    }
    fn expire(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: InstantOf<Q>,
        _: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn on_events(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: Triggered<'_>,
        _: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn before_calls(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: &Waker,
        _: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn aborted(_: &mut RootNode<Q>, _: X, _: AbortCause, _: &Outbox) {}
    fn end(_: &mut RootNode<Q>, _: X, _: &EndReasonOf<TargetOf<Q, X>>, _: &Outbox) {}
    fn remains(_: &mut RootNode<Q>, _: X, _: &mut HNil, _: &Outbox) {}
}
impl<Q, X, O, Ps, SP, PL> PartsOps<Q, X, HCons<O, Ps>> for HCons<SP, PL>
where
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + Copy,
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
        args: &ArgsOf<P_<Q>>,
        outbox: &Outbox,
    ) -> Result<HCons<O::Data, <Ps as Shares<TargetOf<Q, X>>>::Data>, LendError> {
        Ok(HCons {
            head: SP::build(root, parent, args, outbox)?,
            tail: PL::build(root, parent, args, outbox)?,
        })
    }
    fn deadline(root: &mut RootNode<Q>, at: X, c: usize, o: &Outbox) -> Option<InstantOf<Q>> {
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
        o: &Outbox,
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
        o: &Outbox,
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
        o: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        match c.checked_sub(SP::LEN) {
            None => SP::before_calls(root, at, c, waker, o).flatten(),
            Some(c) => PL::before_calls(root, at, c, waker, o),
        }
    }
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &Outbox) {
        SP::aborted(root, at, cause, o);
        PL::aborted(root, at, cause, o);
    }
    fn end(root: &mut RootNode<Q>, at: X, reason: &EndReasonOf<TargetOf<Q, X>>, o: &Outbox) {
        SP::end(root, at, reason, o);
        PL::end(root, at, reason, o);
    }
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut HCons<O::Remains, RemainsOf<Ps, TargetOf<Q, X>>>,
        o: &Outbox,
    ) {
        SP::remains(root, at, &mut out.head, o);
        PL::remains(root, at, &mut out.tail, o);
    }
}

// ---------------------------------------------------------------- a place's hooks

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
pub trait ChildPath<R: KindFor<P>, P>: PathFrom<R> + ParentPath {
    /// The parent's path, from its address.
    fn parent_path(parent: &Address<R>) -> Option<ParentOf<Self>>;
    /// Keep `node` under the parent at `parent`, under `key` (a key of
    /// this path's store); `None` when the key is taken.
    fn attach<KK: 'static>(
        root: &mut Node<R, P>,
        parent: &Address<R>,
        key: KK,
        node: Node<Self::Target, P>,
    ) -> Option<Address<R>>
    where
        Self::Target: KindFor<P>;
    /// The node kept under `key` under the parent at `parent`.
    fn find_under<KK: 'static>(
        root: &Node<R, P>,
        parent: &Address<R>,
        key: &KK,
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
    fn attach<KK: 'static>(
        _: &mut Node<R, P>,
        _: &Address<R>,
        _: KK,
        _: Node<R, P>,
    ) -> Option<Address<R>>
    where
        R: KindFor<P>,
    {
        None
    }
    fn find_under<KK: 'static>(_: &Node<R, P>, _: &Address<R>, _: &KK) -> Option<Address<R>> {
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
    fn attach<KK: 'static>(
        root: &mut Node<R, P>,
        parent: &Address<R>,
        key: KK,
        node: Node<Self::Target, P>,
    ) -> Option<Address<R>>
    where
        Self::Target: KindFor<P>,
    {
        let pp = ParentPathOf::<Self>::from_address(parent)?;
        let parent = pp.node_mut(root)?;
        let key = cast(key).expect(SAME_TYPES);
        let store =
            <LastChildren<R, Self> as ChildAt<IndexOf<Self>, P>>::store_mut(&mut parent.children);
        let h = store.insert(key, node).ok()?;
        Some(pp.append(h).address())
    }
    fn find_under<KK: 'static>(
        root: &Node<R, P>,
        parent: &Address<R>,
        key: &KK,
    ) -> Option<Address<R>> {
        let pp = ParentPathOf::<Self>::from_address(parent)?;
        let parent = pp.node(root)?;
        let key = cast_ref(key).expect(SAME_TYPES);
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
    fn info() -> PlaceInfo;
    /// The new node's components, from the parent at `parent` (the root
    /// has none: `root` is `None` then).
    fn build(
        root: Option<&mut RootNode<Q>>,
        parent: &ParentOf<Pl::At>,
        args: &ArgsOf<P_<Q>>,
        outbox: &Outbox,
    ) -> Result<Node<TargetOf<Q, Pl::At>, P_<Q>>, LendError>
    where
        TargetOf<Q, Pl::At>: KindFor<P_<Q>>;
    fn deadline(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        o: &Outbox,
    ) -> Option<InstantOf<Q>>;
    fn expire(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        now: InstantOf<Q>,
        o: &Outbox,
    ) -> Option<RequestsOf<Q>>;
    fn on_events(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        events: Triggered<'_>,
        o: &Outbox,
    ) -> Option<RequestsOf<Q>>;
    fn before_calls(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        waker: &Waker,
        o: &Outbox,
    ) -> Option<RequestsOf<Q>>;
    fn aborted(root: &mut RootNode<Q>, at: &AddressOf<Q>, cause: AbortCause, o: &Outbox);
    /// Every component's end, for `reason`.
    fn end(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        reason: &EndReasonOf<TargetOf<Q, Pl::At>>,
        o: &Outbox,
    );
    /// Every component's end, for the kind's default reason.
    fn end_default(root: &mut RootNode<Q>, at: &AddressOf<Q>, o: &Outbox);
    fn remains(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        out: &mut RemainsOf<P_<Q>, TargetOf<Q, Pl::At>>,
        o: &Outbox,
    ) where
        P_<Q>: RemainsList<TargetOf<Q, Pl::At>>;
    /// Take the node out of its parent's store.
    fn detach(root: &mut RootNode<Q>, at: &AddressOf<Q>) -> bool;
    /// Build a node of the place under the parent at `parent`, under `key`
    /// (of the place's store), and keep it there; `None` when the key is
    /// taken: nothing is built.
    fn add<KK: 'static>(
        root: &mut RootNode<Q>,
        parent: &AddressOf<Q>,
        key: KK,
        args: &ArgsOf<P_<Q>>,
        o: &Outbox,
    ) -> Option<AddressOf<Q>>;
    /// The node of the place kept under `key` under the parent at
    /// `parent`.
    fn find_child<KK: 'static>(
        root: &RootNode<Q>,
        parent: &AddressOf<Q>,
        key: &KK,
    ) -> Option<AddressOf<Q>>;
    /// Move the node at `at` under the node at `to`, of the place of its
    /// parent.
    fn move_under(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        to: &AddressOf<Q>,
    ) -> Result<AddressOf<Q>, MoveError>;
    /// Every component's end, for `reason`, of the type of the place's
    /// kind's reason.
    fn end_with<RR: 'static>(root: &mut RootNode<Q>, at: &AddressOf<Q>, reason: &RR, o: &Outbox);
}
impl<Q, X, PL> PlaceOps<Q, Place<X>> for PlacePositions<PL>
where
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>>
        + FromAddress<RootOf<Q>>
        + ChainOf<RootOf<Q>, HNil>
        + ParentPath
        + ParentPlace
        + Detach<RootOf<Q>, P_<Q>>
        + ChildPath<RootOf<Q>, P_<Q>>
        + Copy
        + 'static,
    <TargetOf<Q, X> as NodeKind>::Children: crate::structure::ChildStores<P_<Q>>,
    ChainAt<Q, X>: OwnPlace,
    P_<Q>: Shares<TargetOf<Q, X>> + RemainsList<TargetOf<Q, X>>,
    PL: PartsOps<Q, X, P_<Q>>,
{
    fn info() -> PlaceInfo {
        let mut comps = Vec::new();
        PL::info(&mut comps);
        PlaceInfo {
            path: TypeId::of::<X>(),
            parent: X::parent_place(),
            index: X::child_index(),
            kind: TypeId::of::<TargetOf<Q, X>>(),
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
        args: &ArgsOf<P_<Q>>,
        outbox: &Outbox,
    ) -> Result<Node<TargetOf<Q, X>, P_<Q>>, LendError> {
        let mut root = root;
        Ok(Node::new(PL::build(&mut root, parent, args, outbox)?))
    }
    fn deadline(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        o: &Outbox,
    ) -> Option<InstantOf<Q>> {
        PL::deadline(root, X::from_address(at)?, c, o)
    }
    fn expire(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        now: InstantOf<Q>,
        o: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        PL::expire(root, X::from_address(at)?, c, now, o)
    }
    fn on_events(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        events: Triggered<'_>,
        o: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        PL::on_events(root, X::from_address(at)?, c, events, o)
    }
    fn before_calls(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        c: usize,
        waker: &Waker,
        o: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        PL::before_calls(root, X::from_address(at)?, c, waker, o)
    }
    fn aborted(root: &mut RootNode<Q>, at: &AddressOf<Q>, cause: AbortCause, o: &Outbox) {
        if let Some(at) = X::from_address(at) {
            PL::aborted(root, at, cause, o);
        }
    }
    fn end(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        reason: &EndReasonOf<TargetOf<Q, X>>,
        o: &Outbox,
    ) {
        if let Some(at) = X::from_address(at) {
            PL::end(root, at, reason, o);
        }
    }
    fn end_default(root: &mut RootNode<Q>, at: &AddressOf<Q>, o: &Outbox) {
        Self::end(root, at, &Default::default(), o);
    }
    fn remains(
        root: &mut RootNode<Q>,
        at: &AddressOf<Q>,
        out: &mut RemainsOf<P_<Q>, TargetOf<Q, X>>,
        o: &Outbox,
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
    fn add<KK: 'static>(
        root: &mut RootNode<Q>,
        parent: &AddressOf<Q>,
        key: KK,
        args: &ArgsOf<P_<Q>>,
        o: &Outbox,
    ) -> Option<AddressOf<Q>> {
        if X::find_under(root, parent, &key).is_some() {
            return None;
        }
        let pp = X::parent_path(parent)?;
        let node = Self::build(Some(root), &pp, args, o).ok()?;
        X::attach(root, parent, key, node)
    }
    fn find_child<KK: 'static>(
        root: &RootNode<Q>,
        parent: &AddressOf<Q>,
        key: &KK,
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
    fn end_with<RR: 'static>(root: &mut RootNode<Q>, at: &AddressOf<Q>, reason: &RR, o: &Outbox) {
        Self::end(root, at, cast_ref(reason).expect(SAME_TYPES), o);
    }
}

/// The end of a pack of places: never reached.
pub struct NoPlacePos;
const NO_PLACE: &str =
    "the end of a pack of places is never reached: the driver walks the places it has";
impl<Q: DriverComposition> PlaceOps<Q, NoPlace> for NoPlacePos {
    fn info() -> PlaceInfo {
        unreachable!("{NO_PLACE}")
    }
    fn build(
        _: Option<&mut RootNode<Q>>,
        _: &(),
        _: &ArgsOf<P_<Q>>,
        _: &Outbox,
    ) -> Result<RootNode<Q>, LendError> {
        unreachable!("{NO_PLACE}")
    }
    fn deadline(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: usize,
        _: &Outbox,
    ) -> Option<InstantOf<Q>> {
        None
    }
    fn expire(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: usize,
        _: InstantOf<Q>,
        _: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn on_events(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: usize,
        _: Triggered<'_>,
        _: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn before_calls(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: usize,
        _: &Waker,
        _: &Outbox,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn aborted(_: &mut RootNode<Q>, _: &AddressOf<Q>, _: AbortCause, _: &Outbox) {}
    fn end(_: &mut RootNode<Q>, _: &AddressOf<Q>, _: &EndReasonOf<RootOf<Q>>, _: &Outbox) {}
    fn end_default(_: &mut RootNode<Q>, _: &AddressOf<Q>, _: &Outbox) {}
    fn remains(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: &mut RemainsOf<P_<Q>, RootOf<Q>>,
        _: &Outbox,
    ) where
        P_<Q>: RemainsList<RootOf<Q>>,
    {
    }
    fn detach(_: &mut RootNode<Q>, _: &AddressOf<Q>) -> bool {
        false
    }
    fn add<KK: 'static>(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: KK,
        _: &ArgsOf<P_<Q>>,
        _: &Outbox,
    ) -> Option<AddressOf<Q>> {
        None
    }
    fn find_child<KK: 'static>(_: &RootNode<Q>, _: &AddressOf<Q>, _: &KK) -> Option<AddressOf<Q>> {
        None
    }
    fn move_under(
        _: &mut RootNode<Q>,
        _: &AddressOf<Q>,
        _: &AddressOf<Q>,
    ) -> Result<AddressOf<Q>, MoveError> {
        Err(MoveError::NoSource)
    }
    fn end_with<RR: 'static>(_: &mut RootNode<Q>, _: &AddressOf<Q>, _: &RR, _: &Outbox) {}
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

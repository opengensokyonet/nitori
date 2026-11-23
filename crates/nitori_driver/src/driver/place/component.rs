//! One component at a place: lending what its `build` reads at the parent,
//! the positions of its hooks, and the lends of its hooks.
use alloc::vec::Vec;
use core::{any::TypeId, marker::PhantomData};

use frunk_core::hlist::HNil;

use super::{ParentOf, PlaceKey, PlaceKeyOf, PlacesUnder, RootNode, SelfParts, TargetOf};

use crate::{
    call::{ChainAt, Composition, LendInstant, OutboxOf, PartsOf, RootOf},
    event::{Outbox, Subscription, Subscriptions},
    hooks::{BuildKeyed, BuildView, HookKeys, HookView, Hooks, OwnNeeds},
    lend::{Flat, FromSplit, GivenKey, KeyedList, LendAt, LendError, NeedSet, ViewOf, Writes},
    need::ChainOf,
    request::{IntoRequestSum, PlaceRequests, RequestSumOf},
    structure::{End, KindFor, Node, ParentPath, PathFrom, Step},
};

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
pub(super) type P_<Q> = PartsOf<Q>;
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

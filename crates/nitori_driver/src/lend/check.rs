//! The checks of a list of needs, with their reasons: how many nodes have
//! a component, and whether two needs overlap.
//!
//! Finding a node by its components is inference, which says only that it
//! could not decide when no node or two nodes have the component. The
//! checks here count by the components' numbers ([`Numbered`]) and report
//! the count, and are bounds of the entry next to the proof of the needs,
//! so their reasons are what is reported.
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

use super::{
    list::{Append, IncIf, Len, Or, RouteEq, S, Sum, Z},
    own::KeyIs,
    tree::{
        AddressPos, AtMember, AtPos, Ctx, DepthGe, FindBelow, FindUp, KeyPos, MemberPos, On,
        UnderPos, UpPos,
    },
    under::{MCx, MRoute, UnderBody},
    up::{AtCx, UpKind, UpPre},
};
use crate::{
    id::{False, NumberEq, Numbered, True},
    need::{At, Mut, OwnAddress, OwnKey, Ref, Under},
    structure::{Child, NodeKind, Shares},
};

// ------------------------------------------------------------------ counting

/// How many of the components `Self` are `X`.
pub trait CountIn<X> {
    /// The count.
    type N;
}
impl<X> CountIn<X> for HNil {
    type N = Z;
}
impl<X: Numbered, C: Numbered, T: CountIn<X>> CountIn<X> for HCons<C, T>
where
    C::Number: NumberEq<X::Number>,
    <C::Number as NumberEq<X::Number>>::Out: IncIf<T::N>,
{
    type N = <<C::Number as NumberEq<X::Number>>::Out as IncIf<T::N>>::Out;
}

/// How many nodes of the chain `Self` have `X`.
pub trait CountChain<X> {
    /// The count.
    type N;
}
impl<X> CountChain<X> for HNil {
    type N = Z;
}
impl<X, K, Cs: CountIn<X>, F, Pre, St, T: CountChain<X>> CountChain<X>
    for HCons<On<K, Cs, F, Pre, St>, T>
where
    Cs::N: Sum<T::N>,
{
    type N = <Cs::N as Sum<T::N>>::Out;
}

/// How many kinds at or below the kind `Self` have `X`, for the parts `Pt`.
pub trait CountTree<Pt, X> {
    /// The count.
    type N;
}
impl<Pt, X, K: NodeKind> CountTree<Pt, X> for K
where
    Pt: Shares<K>,
    Pt::Data: super::tree::FlatData,
    super::tree::Comps<K, Pt>: CountIn<X>,
    K::Children: CountKids<Pt, X>,
    <super::tree::Comps<K, Pt> as CountIn<X>>::N: Sum<<K::Children as CountKids<Pt, X>>::N>,
{
    type N = <<super::tree::Comps<K, Pt> as CountIn<X>>::N as Sum<
        <K::Children as CountKids<Pt, X>>::N,
    >>::Out;
}
/// How many kinds at or below a list of children have `X`.
pub trait CountKids<Pt, X> {
    /// The count.
    type N;
}
impl<Pt, X> CountKids<Pt, X> for HNil {
    type N = Z;
}
impl<Pt, X, C: CountTree<Pt, X>, St, T: CountKids<Pt, X>> CountKids<Pt, X>
    for HCons<Child<C, St>, T>
where
    C::N: Sum<T::N>,
{
    type N = <C::N as Sum<T::N>>::Out;
}

/// No node.
pub struct Zero;
/// One node.
pub struct One;
/// Two nodes.
pub struct Two;
/// More than two nodes.
pub struct Many;
/// A count as it is reported.
pub trait Spell {
    /// [`Zero`], [`One`], [`Two`] or [`Many`].
    type Name;
}
impl Spell for Z {
    type Name = Zero;
}
impl Spell for S<Z> {
    type Name = One;
}
impl Spell for S<S<Z>> {
    type Name = Two;
}
impl<N> Spell for S<S<S<N>>> {
    type Name = Many;
}

/// Exactly one node from the lend point up to the root has `X`.
#[diagnostic::on_unimplemented(
    message = "taking `{X}` on the way up: `{Self}` of the nodes from the lend point up to the root have it, and exactly one must",
    label = "no node, or more than one, on the way up has this component",
    note = "`Mut<X>`, `Ref<X>`, the anchor of `Under<X, ..>` and `At<X, ..>` take the one node from the lend point up to the root that has `X`"
)]
pub trait ExactlyOneUp<X> {}
impl<X> ExactlyOneUp<X> for One {}

/// Exactly one kind at or below the anchor has `X`.
#[diagnostic::on_unimplemented(
    message = "taking `{X}` below the anchor: `{Self}` of the kinds at or below it have it, and exactly one must",
    label = "no kind, or more than one, below the anchor has this component",
    note = "the members of `Under<A, T>` are the nodes of the one kind, at or below the anchor, that has the component `T` takes"
)]
pub trait ExactlyOneBelow<X> {}
impl<X> ExactlyOneBelow<X> for One {}

type UpCount<C, X> = <<<C as Ctx>::Chain as CountChain<X>>::N as Spell>::Name;

// ------------------------------------------------------------------ overlap

/// What a need takes: the component `X` of the kind at the route `Rt`,
/// exclusive when `E` is [`True`].
pub struct Touch<Rt, X, E>(PhantomData<fn() -> (Rt, X, E)>);

/// Two takings that clash: the same component of the same kind, one of
/// them exclusive; `Self` says whether they do.
#[diagnostic::on_unimplemented(
    message = "two needs overlap: both take `{X}` of the same kind of node, and one of them exclusive",
    label = "these needs take the same component",
    note = "needs in one list are disjoint by kind and component; two `Ref`s of a component do not overlap, a `Mut` and anything else of it do, on any nodes of the kind"
)]
pub trait Apart<X> {}
impl<X> Apart<X> for False {}

/// Whether two takings clash.
pub trait Clash<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl<R1, X1: Numbered, E1, R2, X2: Numbered, E2> Clash<Touch<R2, X2, E2>> for Touch<R1, X1, E1>
where
    R1: RouteEq<R2>,
    X1::Number: NumberEq<X2::Number>,
    E1: Or<E2>,
    <X1::Number as NumberEq<X2::Number>>::Out: super::list::And<<E1 as Or<E2>>::Out>,
    R1::Out: super::list::And<
            <<X1::Number as NumberEq<X2::Number>>::Out as super::list::And<<E1 as Or<E2>>::Out>>::Out,
        >,
{
    type Out = <R1::Out as super::list::And<
        <<X1::Number as NumberEq<X2::Number>>::Out as super::list::And<<E1 as Or<E2>>::Out>>::Out,
    >>::Out;
}

/// No two takings of a list clash.
pub trait Disjoint {}
impl Disjoint for HNil {}
impl<H, T: DisjointFrom<H> + Disjoint> Disjoint for HCons<H, T> {}
/// No taking of a list clashes with `E`.
pub trait DisjointFrom<E> {}
impl<E> DisjointFrom<E> for HNil {}
impl<R1, X1, E1, H, T> DisjointFrom<Touch<R1, X1, E1>> for HCons<H, T>
where
    Touch<R1, X1, E1>: Clash<H>,
    <Touch<R1, X1, E1> as Clash<H>>::Out: Apart<X1>,
    T: DisjointFrom<Touch<R1, X1, E1>>,
{
}

// ------------------------------------------------------------------ the check

/// The checks of the need `Self` at the lend point `C` and position `P`:
/// each condition is a bound of its own, so a failing one is reported with
/// its reason.
pub trait Check<C, P> {}
impl<C> Check<C, HNil> for HNil {}
impl<C, N: Check<C, PN>, T: Check<C, PT>, PN, PT> Check<C, HCons<PN, PT>> for HCons<N, T> {}
impl<C: Ctx, X, I> Check<C, UpPos<I>> for Mut<X>
where
    C::Chain: CountChain<X>,
    <C::Chain as CountChain<X>>::N: Spell,
    UpCount<C, X>: ExactlyOneUp<X>,
    I: DepthGe<C::Depth>,
{
}
impl<C: Ctx, X, I> Check<C, UpPos<I>> for Ref<X>
where
    C::Chain: CountChain<X>,
    <C::Chain as CountChain<X>>::N: Spell,
    UpCount<C, X>: ExactlyOneUp<X>,
{
}
impl<C: Ctx, B, T, I, PT> Check<C, AtPos<I, PT>> for At<B, T>
where
    C::Chain: CountChain<B>,
    <C::Chain as CountChain<B>>::N: Spell,
    UpCount<C, B>: ExactlyOneUp<B>,
    Self: CheckAt<C, I, PT>,
{
}
/// The checks of what an `At` takes at its new lend point, once that is
/// found.
pub trait CheckAt<C, I, PT> {}
impl<C: Ctx, B, T, I, PT> CheckAt<C, I, PT> for At<B, T>
where
    C::Chain: FindUp<B, I>,
    AtCx<C, B, I>: Ctx,
    T: Check<AtCx<C, B, I>, PT>,
{
}
impl<C: Ctx, A, Bd, IA, IK, Sh, PB> Check<C, UnderPos<IA, IK, Sh, PB>> for Under<A, Bd>
where
    C::Chain: CountChain<A>,
    <C::Chain as CountChain<A>>::N: Spell,
    UpCount<C, A>: ExactlyOneUp<A>,
    IA: DepthGe<C::Depth>,
    Self: CheckBelow<C, IA, IK, Sh, PB>,
{
}
/// The checks of an `Under` below its anchor, once that is found.
pub trait CheckBelow<C, IA, IK, Sh, PB> {}
impl<C: Ctx, A, Bd, IA, IK, Sh, PB> CheckBelow<C, IA, IK, Sh, PB> for Under<A, Bd>
where
    Bd: UnderBody,
    C::Chain: FindUp<A, IA>,
    UpKind<C, A, IA>: CountTree<C::Parts, Bd::B>,
    <UpKind<C, A, IA> as CountTree<C::Parts, Bd::B>>::N: Spell,
    <<UpKind<C, A, IA> as CountTree<C::Parts, Bd::B>>::N as Spell>::Name: ExactlyOneBelow<Bd::B>,
    Self: CheckMembers<C, IA, IK, Sh, PB>,
{
}
/// The checks of what an `Under` takes at each member, once the members'
/// kind is found.
pub trait CheckMembers<C, IA, IK, Sh, PB> {}
impl<C: Ctx, A, Bd, IA, IK, Sh, PB> CheckMembers<C, IA, IK, Sh, PB> for Under<A, Bd>
where
    Bd: UnderBody,
    C::Chain: FindUp<A, IA>,
    UpKind<C, A, IA>: FindBelow<C::Parts, Bd::B, IK, Sh = Sh>,
    UpPre<C, A, IA>: Append<<UpKind<C, A, IA> as FindBelow<C::Parts, Bd::B, IK>>::Rel>,
    MRoute<C, A, IA, Bd::B, IK>: Len,
    MCx<C, A, IA, Bd::B, IK>: Ctx,
    Bd: CheckBody<MCx<C, A, IA, Bd::B, IK>, PB>,
{
}
impl<C: Ctx, K> Check<C, KeyPos> for OwnKey<K> where C::Storage: KeyIs<K> {}
impl<C: Ctx, A> Check<C, AddressPos<A>> for OwnAddress {}

/// The checks of the need taken at each member, with the member as the
/// lend point `MC`.
pub trait CheckBody<MC, P> {}
impl<MC: Ctx, X, J> CheckBody<MC, MemberPos<J>> for Mut<X> {}
impl<MC: Ctx, X, J> CheckBody<MC, MemberPos<J>> for Ref<X> {}
impl<MC, B, T: Check<MC, PT>, PT> CheckBody<MC, AtPos<AtMember, PT>> for At<B, T> {}

// ------------------------------------------------------------------ what is taken

/// What the need `Self` takes at the lend point `C` and position `P`
/// ([`Touch`]es), for the check of overlaps.
pub trait Takes<C, P> {
    /// The takings.
    type Touches;
}
impl<C> Takes<C, HNil> for HNil {
    type Touches = HNil;
}
impl<C, N: Takes<C, PN>, T: Takes<C, PT>, PN, PT> Takes<C, HCons<PN, PT>> for HCons<N, T>
where
    N::Touches: Append<T::Touches>,
{
    type Touches = <N::Touches as Append<T::Touches>>::Out;
}
impl<C: Ctx, X, I> Takes<C, UpPos<I>> for Mut<X>
where
    C::Chain: FindUp<X, I>,
{
    type Touches = HCons<Touch<UpPre<C, X, I>, X, True>, HNil>;
}
impl<C: Ctx, X, I> Takes<C, UpPos<I>> for Ref<X>
where
    C::Chain: FindUp<X, I>,
{
    type Touches = HCons<Touch<UpPre<C, X, I>, X, False>, HNil>;
}
impl<C: Ctx, B, T, I, PT> Takes<C, AtPos<I, PT>> for At<B, T>
where
    C::Chain: FindUp<B, I>,
    AtCx<C, B, I>: Ctx,
    T: Takes<AtCx<C, B, I>, PT>,
{
    type Touches = T::Touches;
}
impl<C: Ctx, A, Bd, IA, IK, Sh, PB> Takes<C, UnderPos<IA, IK, Sh, PB>> for Under<A, Bd>
where
    Bd: UnderBody,
    C::Chain: FindUp<A, IA>,
    UpKind<C, A, IA>: FindBelow<C::Parts, Bd::B, IK, Sh = Sh>,
    UpPre<C, A, IA>: Append<<UpKind<C, A, IA> as FindBelow<C::Parts, Bd::B, IK>>::Rel>,
    MRoute<C, A, IA, Bd::B, IK>: Len,
    MCx<C, A, IA, Bd::B, IK>: Ctx,
    Bd: TakesBody<MCx<C, A, IA, Bd::B, IK>, PB>,
{
    type Touches = <Bd as TakesBody<MCx<C, A, IA, Bd::B, IK>, PB>>::Touches;
}
impl<C: Ctx, K> Takes<C, KeyPos> for OwnKey<K> {
    type Touches = HNil;
}
impl<C: Ctx, A> Takes<C, AddressPos<A>> for OwnAddress {
    type Touches = HNil;
}

/// What the need taken at each member takes.
pub trait TakesBody<MC, P> {
    /// The takings.
    type Touches;
}
impl<MC: Ctx, X, J> TakesBody<MC, MemberPos<J>> for Mut<X> {
    type Touches = HCons<Touch<MC::Route, X, True>, HNil>;
}
impl<MC: Ctx, X, J> TakesBody<MC, MemberPos<J>> for Ref<X> {
    type Touches = HCons<Touch<MC::Route, X, False>, HNil>;
}
impl<MC, B, T: Takes<MC, PT>, PT> TakesBody<MC, AtPos<AtMember, PT>> for At<B, T> {
    type Touches = T::Touches;
}

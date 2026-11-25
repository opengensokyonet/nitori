//! Reaction points lent: the reactions to a point gathered from the shares
//! of the kinds around the lend point, taken as the branches of one `+`
//! (with only the root on the route, [`Whole`]), and run in turn.
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

use super::{
    Lend,
    check::{Check, Takes, TakesExclusive},
    list::Append,
    plan::Plan,
    plus::{
        CatCols, CheckBranches, Dedup, DisjointBranches, OneEach, Pick, PlusHeld, PlusPos,
        TakesBranches, Union, Whole, branch,
    },
    tree::{Ctx, On},
};
use crate::{
    id::{False, NumberEq, Numbered, True},
    need::Pos,
    react::{AboveLendPoint, LentReactions, Point, Reaction, Reactions, ReactsTo, WithLendPoint},
    structure::{Child, NodeKind, ShareAt},
};

// ------------------------------------------------------------------ the kinds

/// The kinds of the nodes of a chain, the first first.
pub trait ChainKinds {
    /// The kinds.
    type Kinds;
}
impl ChainKinds for HNil {
    type Kinds = HNil;
}
impl<K, Cs, F, Pre, St, T: ChainKinds> ChainKinds for HCons<On<K, Cs, F, Pre, St>, T> {
    type Kinds = HCons<K, T::Kinds>;
}
/// A list without its last item.
pub trait DropLast {
    /// The shorter list.
    type Out;
}
impl<H> DropLast for HCons<H, HNil> {
    type Out = HNil;
}
impl<H, H2, T> DropLast for HCons<H, HCons<H2, T>>
where
    HCons<H2, T>: DropLast,
{
    type Out = HCons<H, <HCons<H2, T> as DropLast>::Out>;
}

/// The kinds whose shares give the reactions, from the chain `Ch` of the
/// lend point, the root first.
pub trait ReactionKinds<Ch>: 'static {
    /// The kinds.
    type Kinds;
}
impl<Ch: ChainKinds> ReactionKinds<Ch> for WithLendPoint {
    type Kinds = Ch::Kinds;
}
impl<Ch: ChainKinds> ReactionKinds<Ch> for AboveLendPoint
where
    Ch::Kinds: DropLast,
{
    type Kinds = <Ch::Kinds as DropLast>::Out;
}

/// Every kind at or below the kind `Self`, `Self` first.
pub trait AllKinds {
    /// The kinds.
    type Kinds;
}
impl<K: NodeKind> AllKinds for K
where
    K::Children: KidKinds,
{
    type Kinds = HCons<K, <K::Children as KidKinds>::Kinds>;
}
/// Every kind at or below a list of children.
pub trait KidKinds {
    /// The kinds.
    type Kinds;
}
impl KidKinds for HNil {
    type Kinds = HNil;
}
impl<C: AllKinds, St, T: KidKinds> KidKinds for HCons<Child<C, St>, T>
where
    C::Kinds: Append<T::Kinds>,
{
    type Kinds = <C::Kinds as Append<T::Kinds>>::Out;
}

// ------------------------------------------------------------------ the reactions

/// `HCons<R, Rest>` when `Self` is [`True`], `Rest` otherwise.
pub trait Keep<R, Rest> {
    /// The list.
    type Out;
}
impl<R, Rest> Keep<R, Rest> for True {
    type Out = HCons<R, Rest>;
}
impl<R, Rest> Keep<R, Rest> for False {
    type Out = Rest;
}

/// The reactions to the point `P` in a list of [`ReactsTo`], in order.
pub trait Filter<P> {
    /// The reactions.
    type Out;
}
impl<P> Filter<P> for HNil {
    type Out = HNil;
}
impl<P: Numbered, Q: Numbered, R, T: Filter<P>> Filter<P> for HCons<ReactsTo<Q, R>, T>
where
    Q::Number: NumberEq<P::Number>,
    <Q::Number as NumberEq<P::Number>>::Out: Keep<R, T::Out>,
{
    type Out = <<Q::Number as NumberEq<P::Number>>::Out as Keep<R, T::Out>>::Out;
}

/// The reactions to `P` of the part `Self` of the composition of the parts
/// `L` in the kinds `Ks`, the kinds in order.
pub trait PartReactions<P, Ks, L> {
    /// The reactions.
    type Rs;
}
impl<O, P, L> PartReactions<P, HNil, L> for O {
    type Rs = HNil;
}
impl<O, P, K, Ks, L> PartReactions<P, HCons<K, Ks>, L> for O
where
    O: ShareAt<K, L> + PartReactions<P, Ks, L>,
    O::Reacts: Filter<P>,
    <O::Reacts as Filter<P>>::Out: Append<<O as PartReactions<P, Ks, L>>::Rs>,
{
    type Rs = <<O::Reacts as Filter<P>>::Out as Append<<O as PartReactions<P, Ks, L>>::Rs>>::Out;
}

/// The reactions to `P` of the parts `Self` of the composition of the
/// parts `L` in the kinds `Ks`: the parts in order, and within a part the
/// kinds.
pub trait ListReactions<P, Ks, L> {
    /// The reactions.
    type Rs;
}
impl<P, Ks, L> ListReactions<P, Ks, L> for HNil {
    type Rs = HNil;
}
impl<P, Ks, L, O, Os> ListReactions<P, Ks, L> for HCons<O, Os>
where
    O: PartReactions<P, Ks, L>,
    Os: ListReactions<P, Ks, L>,
    O::Rs: Append<Os::Rs>,
{
    type Rs = <O::Rs as Append<Os::Rs>>::Out;
}

/// The needs of a list of reactions to `P`, one list per reaction.
pub trait ReactionNeeds<P: Point> {
    /// The needs.
    type Needs;
}
impl<P: Point> ReactionNeeds<P> for HNil {
    type Needs = HNil;
}
impl<P: Point, R: Reaction<P>, T: ReactionNeeds<P>> ReactionNeeds<P> for HCons<R, T> {
    type Needs = HCons<R::Needs, T::Needs>;
}

/// The reactions to `P` lent at `C` from the kinds `W` picks.
pub type RsAt<C, P, W> = <<C as Ctx>::Parts as ListReactions<
    P,
    <W as ReactionKinds<<C as Ctx>::Chain>>::Kinds,
    <C as Ctx>::Parts,
>>::Rs;
/// Their needs.
pub type NeedsAt<C, P, W> = <RsAt<C, P, W> as ReactionNeeds<P>>::Needs;

// ------------------------------------------------------------------ running

/// Running a list of reactions to `P` lent at `C`, their needs at the
/// positions `Ps` in the union `U`, one after another.
pub trait RunAll<C: Ctx, P: Point, Ps, U: Union> {
    /// Run every reaction with `payload`, each taking its branch of what
    /// is held.
    fn run_all<'p>(
        held: &mut PlusHeld<'_, U, C::Fk>,
        payload: P::Payload<'p>,
        _: PhantomData<&'p ()>,
    );
}
impl<C: Ctx, P: Point, U: Union> RunAll<C, P, HNil, U> for HNil {
    fn run_all<'p>(_: &mut PlusHeld<'_, U, C::Fk>, _: P::Payload<'p>, _: PhantomData<&'p ()>) {}
}
impl<C: Ctx, P: Point, U: Union, R, T, PR: Pos, PT> RunAll<C, P, HCons<PR, PT>, U> for HCons<R, T>
where
    R: Reaction<P>,
    R::Needs: Lend<C, PR>,
    <R::Needs as Lend<C, PR>>::Cols: Pick<U>,
    T: RunAll<C, P, PT, U>,
{
    fn run_all<'p>(
        held: &mut PlusHeld<'_, U, C::Fk>,
        payload: P::Payload<'p>,
        _: PhantomData<&'p ()>,
    ) {
        R::run::<PR>(branch::<C, R::Needs, PR, U>(held), payload);
        T::run_all(held, payload, PhantomData);
    }
}

// ------------------------------------------------------------------ the need

impl<C, P, W, Ps: Pos, U, F> Lend<C, PlusPos<Ps, U, F>> for Reactions<P, W>
where
    C: Ctx<Fk = F>,
    F: Clone + PartialEq + 'static,
    P: Point,
    W: ReactionKinds<C::Chain>,
    C::Parts: ListReactions<P, W::Kinds, C::Parts>,
    RsAt<C, P, W>: ReactionNeeds<P> + RunAll<C, P, Ps, U>,
    NeedsAt<C, P, W>: CatCols<C, Ps>,
    <NeedsAt<C, P, W> as CatCols<C, Ps>>::Cat: Dedup<Out = U>,
    U: Union,
{
    type Cols = Whole<U>;
    fn build<'a>(hs: <U as Plan>::Hs<'a>, at: &F) -> LentReactions<'a, P, PlusPos<Ps, U, F>>
    where
        Self: 'a,
    {
        LentReactions::new(
            PlusHeld::new(hs, at.clone()),
            <RsAt<C, P, W> as RunAll<C, P, Ps, U>>::run_all,
        )
    }
}
impl<C: Ctx, P, W, Ps, U, F> Check<C, PlusPos<Ps, U, F>> for Reactions<P, W>
where
    P: Point,
    W: ReactionKinds<C::Chain>,
    C::Parts: ListReactions<P, W::Kinds, C::Parts>,
    RsAt<C, P, W>: ReactionNeeds<P>,
    NeedsAt<C, P, W>: CheckBranches<C, Ps> + DisjointBranches<C, Ps>,
{
}
impl<C: Ctx, P, W, Ps, U, F> Takes<C, PlusPos<Ps, U, F>> for Reactions<P, W>
where
    P: Point,
    W: ReactionKinds<C::Chain>,
    C::Parts: ListReactions<P, W::Kinds, C::Parts>,
    RsAt<C, P, W>: ReactionNeeds<P>,
    NeedsAt<C, P, W>: TakesBranches<C, Ps>,
    <NeedsAt<C, P, W> as TakesBranches<C, Ps>>::Touches: OneEach,
{
    type Touches = <<NeedsAt<C, P, W> as TakesBranches<C, Ps>>::Touches as OneEach>::Out;
}
/// What the reactions take follows from the composition, not from the
/// need: taken as exclusive.
impl<P, W> TakesExclusive for Reactions<P, W> {
    type Out = True;
}

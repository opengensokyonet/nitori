//! `+`: what the branches take, lent once as one union, and the handles of
//! a branch picked out of it each time the branch is taken.
//!
//! The union is a flat list of exclusive columns and node sets, each once
//! ([`Norm`], [`Dedup`]); a column any branch takes, exclusive or shared,
//! is in it exclusive. Taking a branch borrows the union again: first the
//! branch's exclusive columns, each lent again for the branch, then the
//! rest read through for its shared columns and node sets ([`Pick`]).
use alloc::vec::Vec;
use core::marker::PhantomData;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::{
    Lend,
    check::{Check, Disjoint, Takes, Touch},
    list::{And, Append, FirstTrue, IndexEq, Len, Nowhere, Or, RouteEq, S, Z},
    plan::{ExCol, NodeSet, Plan, ShCol, Shared},
    tree::Ctx,
};
use crate::{
    id::{False, NumberEq, Numbered, True},
    need::{Branches, Plus, PlusView, Pos},
};

// ------------------------------------------------------------------ the union

/// A plan as the flat list of what it takes, a column as an exclusive one.
pub trait Norm {
    /// The flat list.
    type Out;
}
impl Norm for () {
    type Out = HNil;
}
impl<A: Norm, B: Norm> Norm for (A, B)
where
    A::Out: Append<B::Out>,
{
    type Out = <A::Out as Append<B::Out>>::Out;
}
impl Norm for HNil {
    type Out = HNil;
}
impl<A: Norm, B: Norm> Norm for HCons<A, B>
where
    A::Out: Append<B::Out>,
{
    type Out = <A::Out as Append<B::Out>>::Out;
}
impl<Rt, J, F, X> Norm for ExCol<Rt, J, F, X> {
    type Out = HCons<ExCol<Rt, J, F, X>, HNil>;
}
impl<Rt, J, F, X> Norm for ShCol<Rt, J, F, X> {
    type Out = HCons<ExCol<Rt, J, F, X>, HNil>;
}
impl<Rt, F> Norm for NodeSet<Rt, F> {
    type Out = HCons<NodeSet<Rt, F>, HNil>;
}

/// Whether two items of a flat plan are the same column or node set.
pub trait SameItem<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl<R1: RouteEq<R2>, J1: IndexEq<J2>, F1, X1, R2, J2, F2, X2> SameItem<ExCol<R2, J2, F2, X2>>
    for ExCol<R1, J1, F1, X1>
where
    R1::Out: And<J1::Out>,
{
    type Out = <R1::Out as And<J1::Out>>::Out;
}
impl<R1, J1, F1, X1, R2, F2> SameItem<NodeSet<R2, F2>> for ExCol<R1, J1, F1, X1> {
    type Out = False;
}
impl<R1, F1, R2, J2, F2, X2> SameItem<ExCol<R2, J2, F2, X2>> for NodeSet<R1, F1> {
    type Out = False;
}
impl<R1: RouteEq<R2>, F1, R2, F2> SameItem<NodeSet<R2, F2>> for NodeSet<R1, F1> {
    type Out = R1::Out;
}

/// Whether a flat plan has the item `X`.
pub trait HasItem<X> {
    /// [`True`] or [`False`].
    type Out;
}
impl<X> HasItem<X> for HNil {
    type Out = False;
}
impl<X, H: SameItem<X>, T: HasItem<X>> HasItem<X> for HCons<H, T>
where
    H::Out: Or<T::Out>,
{
    type Out = <H::Out as Or<T::Out>>::Out;
}

/// The list `T`, with `H` before it unless the answer is [`True`].
pub trait KeepUnless<H, T> {
    /// The list.
    type Out;
}
impl<H, T> KeepUnless<H, T> for True {
    type Out = T;
}
impl<H, T> KeepUnless<H, T> for False {
    type Out = HCons<H, T>;
}

/// A flat plan with every item once.
pub trait Dedup {
    /// The plan.
    type Out;
}
impl Dedup for HNil {
    type Out = HNil;
}
impl<H, T> Dedup for HCons<H, T>
where
    T: HasItem<H> + Dedup,
    <T as HasItem<H>>::Out: KeepUnless<H, <T as Dedup>::Out>,
{
    type Out = <<T as HasItem<H>>::Out as KeepUnless<H, <T as Dedup>::Out>>::Out;
}

/// The position of the item `X` in a flat plan.
pub trait IndexOf<X> {
    /// [`Here`], [`There`] or, missing, [`Nowhere`].
    type I;
}
impl<X> IndexOf<X> for HNil {
    type I = Nowhere;
}
impl<X, H: SameItem<X>, T: IndexOf<X>> IndexOf<X> for HCons<H, T>
where
    H::Out: FirstTrue<T::I>,
{
    type I = <H::Out as FirstTrue<T::I>>::Out;
}

// ------------------------------------------------------------------ lending the union again

/// An item of the union, lent again for `'b` from handles lent for `'c`.
pub trait Item: Plan {
    /// The item, exclusive, until a branch takes it.
    type Slot<'b, 'c>
    where
        'c: 'b;
    /// What is left of it, read through.
    type Seen<'b, 'c>: Copy
    where
        'c: 'b;
    /// The item, from its handles.
    fn slot<'b, 'c: 'b>(hs: &'b mut Self::Hs<'c>) -> Self::Slot<'b, 'c>;
    /// What is left of the item.
    fn seen<'b, 'c: 'b>(slot: Self::Slot<'b, 'c>) -> Self::Seen<'b, 'c>;
}
impl<Rt: 'static, J: 'static, F: 'static, X: 'static> Item for ExCol<Rt, J, F, X> {
    type Slot<'b, 'c>
        = Option<&'b mut Vec<(F, &'c mut X)>>
    where
        'c: 'b;
    type Seen<'b, 'c>
        = Option<&'b [(F, &'c mut X)]>
    where
        'c: 'b;
    fn slot<'b, 'c: 'b>(hs: &'b mut Vec<(F, &'c mut X)>) -> Self::Slot<'b, 'c> {
        Some(hs)
    }
    fn seen<'b, 'c: 'b>(slot: Self::Slot<'b, 'c>) -> Self::Seen<'b, 'c> {
        slot.map(|v| &v[..])
    }
}
impl<Rt: 'static, F: 'static> Item for NodeSet<Rt, F> {
    type Slot<'b, 'c>
        = &'b [&'c F]
    where
        'c: 'b;
    type Seen<'b, 'c>
        = &'b [&'c F]
    where
        'c: 'b;
    fn slot<'b, 'c: 'b>(hs: &'b mut &'c [&'c F]) -> &'b [&'c F] {
        hs
    }
    fn seen<'b, 'c: 'b>(slot: &'b [&'c F]) -> &'b [&'c F] {
        slot
    }
}

/// The union, lent again item by item.
pub trait Union: Plan {
    /// Every item, exclusive, until a branch takes it.
    type Slots<'b, 'c>
    where
        'c: 'b;
    /// What is left, read through.
    type Seen<'b, 'c>: Copy
    where
        'c: 'b;
    /// Every item, from the handles.
    fn slots<'b, 'c: 'b>(hs: &'b mut Self::Hs<'c>) -> Self::Slots<'b, 'c>;
    /// What is left.
    fn seen<'b, 'c: 'b>(slots: Self::Slots<'b, 'c>) -> Self::Seen<'b, 'c>;
}
impl Union for HNil {
    type Slots<'b, 'c>
        = HNil
    where
        'c: 'b;
    type Seen<'b, 'c>
        = HNil
    where
        'c: 'b;
    fn slots<'b, 'c: 'b>(_: &'b mut HNil) -> HNil {
        HNil
    }
    fn seen<'b, 'c: 'b>(_: HNil) -> HNil {
        HNil
    }
}
impl<H: Item, T: Union> Union for HCons<H, T> {
    type Slots<'b, 'c>
        = HCons<H::Slot<'b, 'c>, T::Slots<'b, 'c>>
    where
        'c: 'b;
    type Seen<'b, 'c>
        = HCons<H::Seen<'b, 'c>, T::Seen<'b, 'c>>
    where
        'c: 'b;
    fn slots<'b, 'c: 'b>(hs: &'b mut Self::Hs<'c>) -> Self::Slots<'b, 'c> {
        HCons {
            head: H::slot(&mut hs.head),
            tail: T::slots(&mut hs.tail),
        }
    }
    fn seen<'b, 'c: 'b>(slots: Self::Slots<'b, 'c>) -> Self::Seen<'b, 'c> {
        HCons {
            head: H::seen(slots.head),
            tail: T::seen(slots.tail),
        }
    }
}

/// The column of the components `X` of nodes with full keys `F` is in the
/// union at `I`.
pub trait ColAt<I>: Union {
    /// The nodes' full keys.
    type F: Clone + 'static;
    /// The component.
    type X: 'static;
    /// The column, exclusive, taken out of the slots.
    fn ex<'b, 'c: 'b>(slots: &mut Self::Slots<'b, 'c>) -> &'b mut Vec<(Self::F, &'c mut Self::X)>;
    /// The column, read through.
    fn read<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [(Self::F, &'c mut Self::X)];
}
impl<Rt: 'static, J: 'static, F: Clone + 'static, X: 'static, T: Union> ColAt<Here>
    for HCons<ExCol<Rt, J, F, X>, T>
{
    type F = F;
    type X = X;
    fn ex<'b, 'c: 'b>(slots: &mut Self::Slots<'b, 'c>) -> &'b mut Vec<(F, &'c mut X)> {
        // Invariant: a branch's needs were checked not to overlap, so it
        // takes a column exclusive once.
        slots
            .head
            .take()
            .expect("a branch takes a column exclusive once")
    }
    fn read<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [(F, &'c mut X)] {
        // Invariant: as for `ex`, a branch does not take a column both
        // exclusive and shared.
        seen.head
            .expect("a branch does not take a column both exclusive and shared")
    }
}
impl<H: Item, T: ColAt<I>, I> ColAt<There<I>> for HCons<H, T> {
    type F = T::F;
    type X = T::X;
    fn ex<'b, 'c: 'b>(slots: &mut Self::Slots<'b, 'c>) -> &'b mut Vec<(T::F, &'c mut T::X)> {
        T::ex(&mut slots.tail)
    }
    fn read<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [(T::F, &'c mut T::X)] {
        T::read(seen.tail)
    }
}

/// The full keys of the nodes of one kind, `F`, are in the union at `I`.
pub trait SetAt<I>: Union {
    /// The nodes' full keys.
    type F: 'static;
    /// The full keys.
    fn set<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [&'c Self::F];
}
impl<Rt: 'static, F: 'static, T: Union> SetAt<Here> for HCons<NodeSet<Rt, F>, T> {
    type F = F;
    fn set<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [&'c F] {
        seen.head
    }
}
impl<H: Item, T: SetAt<I>, I> SetAt<There<I>> for HCons<H, T> {
    type F = T::F;
    fn set<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [&'c T::F] {
        T::set(seen.tail)
    }
}

/// A branch's plan, its handles picked out of the union `U`.
pub trait Pick<U: Union>: Plan {
    /// What the first pass takes.
    type Ex<'b>;
    /// The first pass: the exclusive columns, taken out of the slots.
    fn ex<'b, 'c: 'b>(slots: &mut U::Slots<'b, 'c>) -> Self::Ex<'b>;
    /// The handles, with what is left read through.
    fn pick<'b, 'c: 'b>(ex: Self::Ex<'b>, seen: U::Seen<'b, 'c>) -> Self::Hs<'b>;
}
impl<U: Union> Pick<U> for () {
    type Ex<'b> = ();
    fn ex<'b, 'c: 'b>(_: &mut U::Slots<'b, 'c>) {}
    fn pick<'b, 'c: 'b>(_: (), _: U::Seen<'b, 'c>) {}
}
impl<U: Union, A: Pick<U>, B: Pick<U>> Pick<U> for (A, B) {
    type Ex<'b> = (A::Ex<'b>, B::Ex<'b>);
    fn ex<'b, 'c: 'b>(slots: &mut U::Slots<'b, 'c>) -> Self::Ex<'b> {
        let a = A::ex(slots);
        (a, B::ex(slots))
    }
    fn pick<'b, 'c: 'b>((a, b): Self::Ex<'b>, seen: U::Seen<'b, 'c>) -> Self::Hs<'b> {
        (A::pick(a, seen), B::pick(b, seen))
    }
}
impl<U: Union> Pick<U> for HNil {
    type Ex<'b> = HNil;
    fn ex<'b, 'c: 'b>(_: &mut U::Slots<'b, 'c>) -> HNil {
        HNil
    }
    fn pick<'b, 'c: 'b>(_: HNil, _: U::Seen<'b, 'c>) -> HNil {
        HNil
    }
}
impl<U: Union, A: Pick<U>, B: Pick<U>> Pick<U> for HCons<A, B> {
    type Ex<'b> = HCons<A::Ex<'b>, B::Ex<'b>>;
    fn ex<'b, 'c: 'b>(slots: &mut U::Slots<'b, 'c>) -> Self::Ex<'b> {
        let head = A::ex(slots);
        HCons {
            head,
            tail: B::ex(slots),
        }
    }
    fn pick<'b, 'c: 'b>(ex: Self::Ex<'b>, seen: U::Seen<'b, 'c>) -> Self::Hs<'b> {
        HCons {
            head: A::pick(ex.head, seen),
            tail: B::pick(ex.tail, seen),
        }
    }
}
impl<U, Rt: 'static, J: 'static, F: Clone + 'static, X: 'static> Pick<U> for ExCol<Rt, J, F, X>
where
    U: IndexOf<ExCol<Rt, J, F, X>> + ColAt<<U as IndexOf<ExCol<Rt, J, F, X>>>::I, F = F, X = X>,
{
    type Ex<'b> = Vec<(F, &'b mut X)>;
    fn ex<'b, 'c: 'b>(slots: &mut U::Slots<'b, 'c>) -> Vec<(F, &'b mut X)> {
        <U as ColAt<_>>::ex(slots)
            .iter_mut()
            .map(|(f, x)| (f.clone(), &mut **x))
            .collect()
    }
    fn pick<'b, 'c: 'b>(ex: Vec<(F, &'b mut X)>, _: U::Seen<'b, 'c>) -> Vec<(F, &'b mut X)> {
        ex
    }
}
impl<U, Rt: 'static, J: 'static, F: Clone + 'static, X: 'static> Pick<U> for ShCol<Rt, J, F, X>
where
    U: IndexOf<ExCol<Rt, J, F, X>> + ColAt<<U as IndexOf<ExCol<Rt, J, F, X>>>::I, F = F, X = X>,
{
    type Ex<'b> = ();
    fn ex<'b, 'c: 'b>(_: &mut U::Slots<'b, 'c>) {}
    fn pick<'b, 'c: 'b>(_: (), seen: U::Seen<'b, 'c>) -> Shared<'b, F, X> {
        Shared::Held(<U as ColAt<_>>::read(seen))
    }
}
impl<U, Rt: 'static, F: 'static> Pick<U> for NodeSet<Rt, F>
where
    U: IndexOf<NodeSet<Rt, F>> + SetAt<<U as IndexOf<NodeSet<Rt, F>>>::I, F = F>,
{
    type Ex<'b> = ();
    fn ex<'b, 'c: 'b>(_: &mut U::Slots<'b, 'c>) {}
    fn pick<'b, 'c: 'b>(_: (), seen: U::Seen<'b, 'c>) -> &'b [&'b F] {
        <U as SetAt<_>>::set(seen)
    }
}

// ------------------------------------------------------------------ the need

/// The position of a `Plus`: its branches' positions `Ps`, the union `U`,
/// and the lend point's full key `F`.
pub struct PlusPos<Ps, U, F>(PhantomData<fn() -> (Ps, U, F)>);
impl<Ps: Pos, U: Plan, F: 'static> Pos for PlusPos<Ps, U, F> {
    type Inner = Ps;
    type Held<'t> = PlusHeld<'t, U, F>;
}

/// What the view of a `Plus` holds: the handles of the union, lent for
/// `'t`, and the lend point's full key.
pub struct PlusHeld<'t, U: Plan, F> {
    hs: U::Hs<'t>,
    at: F,
}

/// The view of the branch `B`, at the position `PB`, from what a `Plus`
/// lent at `C` holds.
fn branch<'t, 'b, C: Ctx, B: Lend<C, PB>, PB: Pos, U: Union>(
    held: &'b mut PlusHeld<'t, U, C::Fk>,
) -> B::View<'b, PB>
where
    B::Cols: Pick<U>,
{
    let PlusHeld { hs, at } = held;
    let mut slots = U::slots(hs);
    let ex = <B::Cols as Pick<U>>::ex(&mut slots);
    let hs = <B::Cols as Pick<U>>::pick(ex, U::seen(slots));
    B::build(hs, at)
}

/// What the branches `Self`, at the positions `Ps`, take at `C`: their
/// plans as one flat list.
pub trait CatCols<C, Ps> {
    /// The flat list, with repeats.
    type Cat;
}
impl<C> CatCols<C, HNil> for HNil {
    type Cat = HNil;
}
impl<C: Ctx, B, T, PB: Pos, PT> CatCols<C, HCons<PB, PT>> for HCons<B, T>
where
    B: Lend<C, PB>,
    B::Cols: Norm,
    T: CatCols<C, PT>,
    <B::Cols as Norm>::Out: Append<T::Cat>,
{
    type Cat = <<B::Cols as Norm>::Out as Append<T::Cat>>::Out;
}

/// The functions that make the branches' views from the union `U`.
pub trait MakeFns<C: Ctx, Ps: Pos, U: Union>: Branches {
    /// One function per branch.
    fn fns<'t>() -> Self::Fns<PlusHeld<'t, U, C::Fk>, Ps>;
}
impl<C: Ctx, U: Union> MakeFns<C, HNil, U> for HNil {
    fn fns<'t>() -> Self::Fns<PlusHeld<'t, U, C::Fk>, HNil> {
        HNil
    }
}
impl<C: Ctx, U: Union, B, T, PB: Pos, PT: Pos> MakeFns<C, HCons<PB, PT>, U> for HCons<B, T>
where
    B: Lend<C, PB>,
    B::Cols: Pick<U>,
    T: MakeFns<C, PT, U>,
{
    fn fns<'t>() -> Self::Fns<PlusHeld<'t, U, C::Fk>, HCons<PB, PT>> {
        HCons {
            head: branch::<C, B, PB, U>,
            tail: T::fns(),
        }
    }
}

impl<C, Bs, Ps: Pos, U, F> Lend<C, PlusPos<Ps, U, F>> for Plus<Bs>
where
    C: Ctx<Fk = F>,
    F: Clone + PartialEq + 'static,
    Bs: CatCols<C, Ps> + MakeFns<C, Ps, U>,
    <Bs as CatCols<C, Ps>>::Cat: Dedup<Out = U>,
    U: Union,
{
    type Cols = U;
    fn build<'a>(hs: U::Hs<'a>, at: &F) -> PlusView<'a, Bs, PlusPos<Ps, U, F>>
    where
        Self: 'a,
    {
        PlusView::new(
            PlusHeld { hs, at: at.clone() },
            <Bs as MakeFns<C, Ps, U>>::fns(),
        )
    }
}

// ------------------------------------------------------------------ checks

/// The checks of each branch.
pub trait CheckBranches<C, Ps> {}
impl<C> CheckBranches<C, HNil> for HNil {}
impl<C, B: Check<C, PB>, T: CheckBranches<C, PT>, PB, PT> CheckBranches<C, HCons<PB, PT>>
    for HCons<B, T>
{
}
/// No two needs of one branch overlap.
pub trait DisjointBranches<C, Ps> {}
impl<C> DisjointBranches<C, HNil> for HNil {}
impl<C, B, T, PB, PT> DisjointBranches<C, HCons<PB, PT>> for HCons<B, T>
where
    B: Takes<C, PB>,
    B::Touches: Disjoint,
    T: DisjointBranches<C, PT>,
{
}
impl<C: Ctx, Bs, Ps, U, F> Check<C, PlusPos<Ps, U, F>> for Plus<Bs>
where
    Bs: CheckBranches<C, Ps>,
    Bs: DisjointBranches<C, Ps>,
    Bs: TakesBranches<C, Ps>,
    <Bs as TakesBranches<C, Ps>>::Touches: AllDeep<C::Depth>,
{
}

/// The node at a route `Self` long is at or below the depth `Sd` of the
/// members being lent, so a `Plus` may take its component `X` exclusive.
#[diagnostic::on_unimplemented(
    message = "a `Plus` inside `Under` takes `{X}` above the members, which share it",
    label = "every member would borrow the same node",
    note = "a `Plus` takes all its branches take exclusive, so inside `Under<A, ..>` its branches take only at each member or below it; `OneAtATime` lends the members one at a time"
)]
pub trait DeepEnough<X, Sd> {}
impl<X, N> DeepEnough<X, Z> for N {}
impl<X, N: DeepEnough<X, M>, M> DeepEnough<X, S<M>> for S<N> {}
/// Every taking of a list is at or below the depth `Sd`.
pub trait AllDeep<Sd> {}
impl<Sd> AllDeep<Sd> for HNil {}
impl<Sd, Rt: Len, X, E, T: AllDeep<Sd>> AllDeep<Sd> for HCons<Touch<Rt, X, E>, T> where
    Rt::N: DeepEnough<X, Sd>
{
}

/// What the branches take, one after another.
pub trait TakesBranches<C, Ps> {
    /// The takings, with repeats.
    type Touches;
}
impl<C> TakesBranches<C, HNil> for HNil {
    type Touches = HNil;
}
impl<C, B: Takes<C, PB>, T: TakesBranches<C, PT>, PB, PT> TakesBranches<C, HCons<PB, PT>>
    for HCons<B, T>
where
    B::Touches: Append<T::Touches>,
{
    type Touches = <B::Touches as Append<T::Touches>>::Out;
}
impl<C, Bs, Ps, U, F> Takes<C, PlusPos<Ps, U, F>> for Plus<Bs>
where
    Bs: TakesBranches<C, Ps>,
    Bs::Touches: Exclusive,
{
    type Touches = <Bs::Touches as Exclusive>::Out;
}

/// Whether a list of takings takes the component `X` of the kind at `R`.
pub trait HasTouch<R, X> {
    /// [`True`] or [`False`].
    type Out;
}
impl<R, X> HasTouch<R, X> for HNil {
    type Out = False;
}
impl<R, X: Numbered, R2: RouteEq<R>, X2: Numbered, E2, T: HasTouch<R, X>> HasTouch<R, X>
    for HCons<Touch<R2, X2, E2>, T>
where
    X2::Number: NumberEq<X::Number>,
    R2::Out: And<<X2::Number as NumberEq<X::Number>>::Out>,
    <R2::Out as And<<X2::Number as NumberEq<X::Number>>::Out>>::Out: Or<T::Out>,
{
    type Out = <<R2::Out as And<<X2::Number as NumberEq<X::Number>>::Out>>::Out as Or<T::Out>>::Out;
}

/// A list of takings as one exclusive taking of each component of each
/// kind.
pub trait Exclusive {
    /// The takings.
    type Out;
}
impl Exclusive for HNil {
    type Out = HNil;
}
impl<R, X, E, T> Exclusive for HCons<Touch<R, X, E>, T>
where
    T: HasTouch<R, X> + Exclusive,
    <T as HasTouch<R, X>>::Out: KeepUnless<Touch<R, X, True>, <T as Exclusive>::Out>,
{
    type Out =
        <<T as HasTouch<R, X>>::Out as KeepUnless<Touch<R, X, True>, <T as Exclusive>::Out>>::Out;
}

//! `+`: what the branches take, lent once as one union, and the handles of
//! a branch picked out of it each time the branch is taken.
//!
//! The union is a flat list of columns and node sets, each once ([`Norm`],
//! [`Dedup`]); a column some branch takes exclusive is in it exclusive, one
//! every branch only reads is in it shared. Taking a branch borrows the
//! union again: first the branch's exclusive columns, each lent again for
//! the branch, then the rest read through for its shared columns and node
//! sets ([`Pick`]).
use core::marker::PhantomData;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::{
    Lend,
    check::{Check, Deep, Disjoint, Takes, TakesExclusive, Touch},
    list::{And, Append, FirstTrue, IndexEq, Nowhere, Or, RouteEq},
    plan::{Anchored, ExCol, NodeSet, Plan, ShCol, Shared},
    rows::{Rows, Span},
    tree::Ctx,
};
use crate::{
    id::{False, NumberEq, Numbered, True},
    need::{Branches, Plus, PlusView, Pos},
};

// ------------------------------------------------------------------ the union

/// A plan as the flat list of what it takes.
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
    type Out = HCons<ShCol<Rt, J, F, X>, HNil>;
}
impl<Rt, F, A> Norm for NodeSet<Rt, F, A> {
    type Out = HCons<NodeSet<Rt, F, A>, HNil>;
}
impl<A, P: Norm> Norm for Anchored<A, P> {
    type Out = P::Out;
}

/// Whether two items of a flat plan are the same column, exclusive or
/// shared, or the same node set.
pub trait SameItem<B> {
    /// [`True`] or [`False`].
    type Out;
}
/// Whether two columns are the same: of the kind at one route, at one
/// position of its components.
pub type SameCol<R1, J1, R2, J2> = <<R1 as RouteEq<R2>>::Out as And<<J1 as IndexEq<J2>>::Out>>::Out;
macro_rules! same_col {
    ($($a:ident $b:ident;)*) => {$(
        impl<R1: RouteEq<R2>, J1: IndexEq<J2>, F1, X1, R2, J2, F2, X2> SameItem<$b<R2, J2, F2, X2>>
            for $a<R1, J1, F1, X1>
        where
            R1::Out: And<J1::Out>,
        {
            type Out = SameCol<R1, J1, R2, J2>;
        }
    )*};
}
same_col! {
    ExCol ExCol;
    ExCol ShCol;
    ShCol ExCol;
    ShCol ShCol;
}
impl<R1, J1, F1, X1, R2, F2, A2> SameItem<NodeSet<R2, F2, A2>> for ExCol<R1, J1, F1, X1> {
    type Out = False;
}
impl<R1, F1, A1, R2, J2, F2, X2> SameItem<ExCol<R2, J2, F2, X2>> for NodeSet<R1, F1, A1> {
    type Out = False;
}
impl<R1, J1, F1, X1, R2, F2, A2> SameItem<NodeSet<R2, F2, A2>> for ShCol<R1, J1, F1, X1> {
    type Out = False;
}
impl<R1, F1, A1, R2, J2, F2, X2> SameItem<ShCol<R2, J2, F2, X2>> for NodeSet<R1, F1, A1> {
    type Out = False;
}
/// Node sets are the same when they are of one kind below one anchor.
impl<R1: RouteEq<R2>, F1, A1: RouteEq<A2>, R2, F2, A2> SameItem<NodeSet<R2, F2, A2>>
    for NodeSet<R1, F1, A1>
where
    R1::Out: And<A1::Out>,
{
    type Out = <R1::Out as And<A1::Out>>::Out;
}

/// One item of a flat plan for two that are the same: a column exclusive
/// when either is.
pub trait Merge<B> {
    /// The item.
    type Out;
}
impl<Rt, J, F, X, B> Merge<B> for ExCol<Rt, J, F, X> {
    type Out = Self;
}
impl<Rt, J, F, X, Rt2, J2, F2, X2> Merge<ExCol<Rt2, J2, F2, X2>> for ShCol<Rt, J, F, X> {
    type Out = ExCol<Rt2, J2, F2, X2>;
}
impl<Rt, J, F, X, Rt2, J2, F2, X2> Merge<ShCol<Rt2, J2, F2, X2>> for ShCol<Rt, J, F, X> {
    type Out = Self;
}
impl<Rt, F, A, B> Merge<B> for NodeSet<Rt, F, A> {
    type Out = Self;
}

/// A flat plan with every item once, merged ([`Merge`]) where it was
/// more than once.
pub trait Dedup {
    /// The plan.
    type Out;
}
impl Dedup for HNil {
    type Out = HNil;
}
impl<H, T: Dedup> Dedup for HCons<H, T>
where
    T::Out: Insert<H>,
{
    type Out = <T::Out as Insert<H>>::Out;
}
/// A flat plan with every item once, with the item `X` put in: merged
/// with the same item, or added at the end.
pub trait Insert<X> {
    /// The plan.
    type Out;
}
impl<X> Insert<X> for HNil {
    type Out = HCons<X, HNil>;
}
impl<X: SameItem<H>, H, T> Insert<X> for HCons<H, T>
where
    X::Out: InsertAt<X, H, T>,
{
    type Out = <X::Out as InsertAt<X, H, T>>::Out;
}
/// [`Insert`] at the item `H` before `T`, by whether `X` is the same
/// item as `H`.
pub trait InsertAt<X, H, T> {
    /// The plan.
    type Out;
}
impl<X: Merge<H>, H, T> InsertAt<X, H, T> for True {
    type Out = HCons<X::Out, T>;
}
impl<X, H, T: Insert<X>> InsertAt<X, H, T> for False {
    type Out = HCons<H, T::Out>;
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
        = Option<&'b mut Rows<(F, &'c mut X, Span)>>
    where
        'c: 'b;
    type Seen<'b, 'c>
        = Option<&'b [(F, &'c mut X, Span)]>
    where
        'c: 'b;
    fn slot<'b, 'c: 'b>(hs: &'b mut Rows<(F, &'c mut X, Span)>) -> Self::Slot<'b, 'c> {
        Some(hs)
    }
    fn seen<'b, 'c: 'b>(slot: Self::Slot<'b, 'c>) -> Self::Seen<'b, 'c> {
        slot.map(|v| &v[..])
    }
}
impl<Rt: 'static, F: 'static, A: 'static> Item for NodeSet<Rt, F, A> {
    type Slot<'b, 'c>
        = &'b [(&'c F, Span)]
    where
        'c: 'b;
    type Seen<'b, 'c>
        = &'b [(&'c F, Span)]
    where
        'c: 'b;
    fn slot<'b, 'c: 'b>(hs: &'b mut &'c [(&'c F, Span)]) -> &'b [(&'c F, Span)] {
        hs
    }
    fn seen<'b, 'c: 'b>(slot: &'b [(&'c F, Span)]) -> &'b [(&'c F, Span)] {
        slot
    }
}

impl<Rt: 'static, J: 'static, F: 'static, X: 'static> Item for ShCol<Rt, J, F, X> {
    type Slot<'b, 'c>
        = Shared<'b, F, X>
    where
        'c: 'b;
    type Seen<'b, 'c>
        = Shared<'b, F, X>
    where
        'c: 'b;
    fn slot<'b, 'c: 'b>(hs: &'b mut Shared<'c, F, X>) -> Shared<'b, F, X> {
        *hs
    }
    fn seen<'b, 'c: 'b>(slot: Shared<'b, F, X>) -> Shared<'b, F, X> {
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
    fn ex<'b, 'c: 'b>(
        slots: &mut Self::Slots<'b, 'c>,
    ) -> &'b mut Rows<(Self::F, &'c mut Self::X, Span)>;
}
impl<Rt: 'static, J: 'static, F: Clone + 'static, X: 'static, T: Union> ColAt<Here>
    for HCons<ExCol<Rt, J, F, X>, T>
{
    type F = F;
    type X = X;
    fn ex<'b, 'c: 'b>(slots: &mut Self::Slots<'b, 'c>) -> &'b mut Rows<(F, &'c mut X, Span)> {
        // Invariant: a branch's needs were checked not to overlap, so it
        // takes a column exclusive once.
        slots
            .head
            .take()
            .expect("a branch takes a column exclusive once")
    }
}
impl<H: Item, T: ColAt<I>, I> ColAt<There<I>> for HCons<H, T> {
    type F = T::F;
    type X = T::X;
    fn ex<'b, 'c: 'b>(slots: &mut Self::Slots<'b, 'c>) -> &'b mut Rows<(T::F, &'c mut T::X, Span)> {
        T::ex(&mut slots.tail)
    }
}

/// The column of the components `X` of nodes with full keys `F` is in the
/// union at `I`, exclusive or shared, and is read there.
pub trait ReadAt<I>: Union {
    /// The nodes' full keys.
    type F: 'static;
    /// The component.
    type X: 'static;
    /// The column, read through.
    fn read<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> Shared<'b, Self::F, Self::X>;
}
impl<Rt: 'static, J: 'static, F: 'static, X: 'static, T: Union> ReadAt<Here>
    for HCons<ExCol<Rt, J, F, X>, T>
{
    type F = F;
    type X = X;
    fn read<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> Shared<'b, F, X> {
        // Invariant: a branch's needs were checked not to overlap, so a
        // branch does not take a column both exclusive and shared.
        Shared::Held(
            seen.head
                .expect("a branch does not take a column both exclusive and shared"),
        )
    }
}
impl<Rt: 'static, J: 'static, F: 'static, X: 'static, T: Union> ReadAt<Here>
    for HCons<ShCol<Rt, J, F, X>, T>
{
    type F = F;
    type X = X;
    fn read<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> Shared<'b, F, X> {
        seen.head
    }
}
impl<H: Item, T: ReadAt<I>, I> ReadAt<There<I>> for HCons<H, T> {
    type F = T::F;
    type X = T::X;
    fn read<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> Shared<'b, T::F, T::X> {
        T::read(seen.tail)
    }
}

/// The full keys of the nodes of one kind, `F`, are in the union at `I`.
pub trait SetAt<I>: Union {
    /// The nodes' full keys.
    type F: 'static;
    /// The full keys.
    fn set<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [(&'c Self::F, Span)];
}
impl<Rt: 'static, F: 'static, A: 'static, T: Union> SetAt<Here> for HCons<NodeSet<Rt, F, A>, T> {
    type F = F;
    fn set<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [(&'c F, Span)] {
        seen.head
    }
}
impl<H: Item, T: SetAt<I>, I> SetAt<There<I>> for HCons<H, T> {
    type F = T::F;
    fn set<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [(&'c T::F, Span)] {
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
    type Ex<'b> = Rows<(F, &'b mut X, Span)>;
    fn ex<'b, 'c: 'b>(slots: &mut U::Slots<'b, 'c>) -> Rows<(F, &'b mut X, Span)> {
        <U as ColAt<_>>::ex(slots)
            .iter_mut()
            .map(|(f, x, s)| (f.clone(), &mut **x, *s))
            .collect()
    }
    fn pick<'b, 'c: 'b>(
        ex: Rows<(F, &'b mut X, Span)>,
        _: U::Seen<'b, 'c>,
    ) -> Rows<(F, &'b mut X, Span)> {
        ex
    }
}
impl<U, Rt: 'static, J: 'static, F: Clone + 'static, X: 'static> Pick<U> for ShCol<Rt, J, F, X>
where
    U: IndexOf<ShCol<Rt, J, F, X>> + ReadAt<<U as IndexOf<ShCol<Rt, J, F, X>>>::I, F = F, X = X>,
{
    type Ex<'b> = ();
    fn ex<'b, 'c: 'b>(_: &mut U::Slots<'b, 'c>) {}
    fn pick<'b, 'c: 'b>(_: (), seen: U::Seen<'b, 'c>) -> Shared<'b, F, X> {
        <U as ReadAt<_>>::read(seen)
    }
}
impl<U, Rt: 'static, F: 'static, A: 'static> Pick<U> for NodeSet<Rt, F, A>
where
    U: IndexOf<NodeSet<Rt, F, A>> + SetAt<<U as IndexOf<NodeSet<Rt, F, A>>>::I, F = F>,
{
    type Ex<'b> = ();
    fn ex<'b, 'c: 'b>(_: &mut U::Slots<'b, 'c>) {}
    fn pick<'b, 'c: 'b>(_: (), seen: U::Seen<'b, 'c>) -> &'b [(&'b F, Span)] {
        <U as SetAt<_>>::set(seen)
    }
}

impl<U: Union, A: 'static, P: Pick<U>> Pick<U> for Anchored<A, P> {
    type Ex<'b> = P::Ex<'b>;
    fn ex<'b, 'c: 'b>(slots: &mut U::Slots<'b, 'c>) -> Self::Ex<'b> {
        P::ex(slots)
    }
    fn pick<'b, 'c: 'b>(ex: Self::Ex<'b>, seen: U::Seen<'b, 'c>) -> Self::Hs<'b> {
        P::pick(ex, seen)
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
impl<'t, U: Plan, F> PlusHeld<'t, U, F> {
    /// What holds the handles `hs` of the union, lent at `at`.
    pub(super) fn new(hs: U::Hs<'t>, at: F) -> Self {
        Self { hs, at }
    }
}

/// The view of the branch `B`, at the position `PB`, from what a `Plus`
/// lent at `C` holds.
pub(super) fn branch<'t, 'b, C: Ctx, B: Lend<C, PB>, PB: Pos, U: Union>(
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
{
}

impl<Bs: TakesExclusive> TakesExclusive for Plus<Bs> {
    type Out = Bs::Out;
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
    Bs::Touches: OneEach,
{
    type Touches = <Bs::Touches as OneEach>::Out;
}

/// Whether two takings are of the same thing: one component of one kind.
pub trait SameTaking<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl<R1, X1: Numbered, E1, R2, X2: Numbered, E2> SameTaking<Touch<R2, X2, E2>> for Touch<R1, X1, E1>
where
    R1: RouteEq<R2>,
    X1::Number: NumberEq<X2::Number>,
    R1::Out: And<<X1::Number as NumberEq<X2::Number>>::Out>,
{
    type Out = <R1::Out as And<<X1::Number as NumberEq<X2::Number>>::Out>>::Out;
}
impl<R1, X1, E1, I, Sd, W> SameTaking<Deep<I, Sd, W>> for Touch<R1, X1, E1> {
    type Out = False;
}
impl<I, Sd, W, B> SameTaking<B> for Deep<I, Sd, W> {
    type Out = False;
}

/// One taking for two of the same thing: exclusive when either is.
pub trait MergeTaking<B> {
    /// The taking.
    type Out;
}
impl<R, X, E1: Or<E2>, R2, X2, E2> MergeTaking<Touch<R2, X2, E2>> for Touch<R, X, E1> {
    type Out = Touch<R, X, E1::Out>;
}

/// A list of takings with one taking of each thing, exclusive when any of
/// its takings is.
pub trait OneEach {
    /// The takings.
    type Out;
}
impl OneEach for HNil {
    type Out = HNil;
}
impl<H, T: OneEach> OneEach for HCons<H, T>
where
    T::Out: AddTaking<H>,
{
    type Out = <T::Out as AddTaking<H>>::Out;
}
/// A list of takings with one taking of each thing, with `X` put in:
/// merged with the taking of the same thing, or added at the end.
pub trait AddTaking<X> {
    /// The takings.
    type Out;
}
impl<X> AddTaking<X> for HNil {
    type Out = HCons<X, HNil>;
}
impl<X: SameTaking<H>, H, T> AddTaking<X> for HCons<H, T>
where
    X::Out: AddTakingAt<X, H, T>,
{
    type Out = <X::Out as AddTakingAt<X, H, T>>::Out;
}
/// [`AddTaking`] at the taking `H` before `T`, by whether `X` is of
/// the same thing as `H`.
pub trait AddTakingAt<X, H, T> {
    /// The takings.
    type Out;
}
impl<X: MergeTaking<H>, H, T> AddTakingAt<X, H, T> for True {
    type Out = HCons<X::Out, T>;
}
impl<X, H, T: AddTaking<X>> AddTakingAt<X, H, T> for False {
    type Out = HCons<H, T::Out>;
}

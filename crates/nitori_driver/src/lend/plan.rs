//! Column plans: what a list of needs takes from the tables, and the
//! handles it is given.
//!
//! A plan is a tree of pairs whose leaves are an exclusive column
//! ([`ExCol`]), a shared column ([`ShCol`]) or the full keys of one kind's
//! nodes ([`NodeSet`]). It is taken in two passes: the exclusive columns
//! first, in the order of the plan, each component taken out of its row;
//! then, with the rows left shared, the shared columns and node sets, any
//! of them more than once. That no two needs take the same column with
//! one of them exclusive is checked on the types where the needs are
//! proved, so a component missing from its row is a broken invariant.
use alloc::vec::Vec;
use core::marker::PhantomData;

use frunk_core::hlist::HNil;

use super::{
    list::HasPrefix,
    table::{KindTab, SlotAt, TabAt},
};

/// The component `X` of every node of the kind at the route `Rt`, at the
/// position `J` of its components, exclusive; the nodes have full keys
/// `F`.
pub struct ExCol<Rt, J, F, X>(PhantomData<fn() -> (Rt, J, F, X)>);
/// The component `X` of every node of the kind at `Rt`, shared.
pub struct ShCol<Rt, J, F, X>(PhantomData<fn() -> (Rt, J, F, X)>);
/// The full keys of every node of the kind at `Rt`.
pub struct NodeSet<Rt, F>(PhantomData<fn() -> (Rt, F)>);

/// A plan and the handles it is given, borrowing the tree for `'a`.
pub trait Plan: 'static {
    /// The handles.
    type Hs<'a>;
}
impl Plan for () {
    type Hs<'a> = ();
}
impl<A: Plan, B: Plan> Plan for (A, B) {
    type Hs<'a> = (A::Hs<'a>, B::Hs<'a>);
}
impl<Rt: 'static, J: 'static, F: 'static, X: 'static> Plan for ExCol<Rt, J, F, X> {
    type Hs<'a> = Vec<(F, &'a mut X)>;
}
impl<Rt: 'static, J: 'static, F: 'static, X: 'static> Plan for ShCol<Rt, J, F, X> {
    type Hs<'a> = &'a [(&'a F, &'a X)];
}
impl<Rt: 'static, F: 'static> Plan for NodeSet<Rt, F> {
    type Hs<'a> = &'a [&'a F];
}

/// Taking a plan from the tables of the root kind `R` for the parts `Pt`,
/// with the context `X` of the lending.
///
/// The tables borrow the tree for `'a`; the second pass borrows the tables
/// for `'b`, and the handles borrow what it gathered for `'c`.
pub trait Run<R: KindTab<Pt, HNil>, Pt: 'static, X>: Plan {
    /// What the first pass takes.
    type Ex<'a>;
    /// What the second pass gathers.
    type Store<'b>;
    /// The first pass: the exclusive columns, taken out of the rows.
    fn take<'a>(tab: &mut R::Table<'a>, cx: &mut X) -> Self::Ex<'a>
    where
        Self: 'a;
    /// The second pass: the shared columns and node sets.
    fn gather<'a: 'b, 'b>(tab: &'b R::Table<'a>) -> Self::Store<'b>
    where
        Self: 'a;
    /// The handles, from both passes.
    fn handles<'a: 'c, 'b: 'c, 'c>(ex: Self::Ex<'a>, store: &'c Self::Store<'b>) -> Self::Hs<'c>
    where
        Self: 'a;
}
impl<R: KindTab<Pt, HNil>, Pt: 'static, X> Run<R, Pt, X> for () {
    type Ex<'a> = ();
    type Store<'b> = ();
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X) -> Self::Ex<'a>
    where
        Self: 'a,
    {
    }
    fn gather<'a: 'b, 'b>(_: &'b R::Table<'a>) -> Self::Store<'b>
    where
        Self: 'a,
    {
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(_: (), _: &'c ()) -> Self::Hs<'c>
    where
        Self: 'a,
    {
    }
}
impl<R: KindTab<Pt, HNil>, Pt: 'static, X, A, B> Run<R, Pt, X> for (A, B)
where
    A: Run<R, Pt, X>,
    B: Run<R, Pt, X>,
{
    type Ex<'a> = (A::Ex<'a>, B::Ex<'a>);
    type Store<'b> = (A::Store<'b>, B::Store<'b>);
    fn take<'a>(tab: &mut R::Table<'a>, cx: &mut X) -> Self::Ex<'a>
    where
        Self: 'a,
    {
        let a = A::take(tab, cx);
        (a, B::take(tab, cx))
    }
    fn gather<'a: 'b, 'b>(tab: &'b R::Table<'a>) -> Self::Store<'b>
    where
        Self: 'a,
    {
        (A::gather(tab), B::gather(tab))
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(
        (a, b): Self::Ex<'a>,
        (sa, sb): &'c Self::Store<'b>,
    ) -> Self::Hs<'c>
    where
        Self: 'a,
    {
        (A::handles(a, sa), B::handles(b, sb))
    }
}
impl<R, Pt: 'static, X, Rt: 'static, J: 'static, F: Clone + 'static, C: 'static> Run<R, Pt, X>
    for ExCol<Rt, J, F, C>
where
    R: TabAt<Pt, HNil, Rt, AtFk = F>,
    R::AtComps: SlotAt<J, C = C>,
{
    type Ex<'a> = Vec<(F, &'a mut C)>;
    type Store<'b> = ();
    fn take<'a>(tab: &mut R::Table<'a>, _: &mut X) -> Vec<(F, &'a mut C)>
    where
        Self: 'a,
    {
        R::rows_mut(tab)
            .iter_mut()
            .map(|(f, row)| {
                // Invariant: the needs were checked not to overlap, so no
                // need before this one took the component.
                let x = <R::AtComps as SlotAt<J>>::slot_mut(row)
                    .take()
                    .expect("an exclusive column is taken once in a lending");
                (f.clone(), x)
            })
            .collect()
    }
    fn gather<'a: 'b, 'b>(_: &'b R::Table<'a>) -> Self::Store<'b>
    where
        Self: 'a,
    {
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(ex: Vec<(F, &'a mut C)>, _: &'c ()) -> Vec<(F, &'c mut C)>
    where
        Self: 'a,
    {
        ex
    }
}
impl<R, Pt: 'static, X, Rt: 'static, J: 'static, F: 'static, C: 'static> Run<R, Pt, X>
    for ShCol<Rt, J, F, C>
where
    R: TabAt<Pt, HNil, Rt, AtFk = F>,
    R::AtComps: SlotAt<J, C = C>,
{
    type Ex<'a> = ();
    type Store<'b> = Vec<(&'b F, &'b C)>;
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X) -> Self::Ex<'a>
    where
        Self: 'a,
    {
    }
    fn gather<'a: 'b, 'b>(tab: &'b R::Table<'a>) -> Vec<(&'b F, &'b C)>
    where
        Self: 'a,
    {
        R::rows(tab)
            .iter()
            .map(|(f, row)| {
                // Invariant: no exclusive need took a column a shared need
                // takes, which the check of overlaps refuses.
                let x = <R::AtComps as SlotAt<J>>::slot(row)
                    .as_deref()
                    .expect("a shared column is not taken exclusive in the same lending");
                (f, x)
            })
            .collect()
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(_: (), store: &'c Vec<(&'b F, &'b C)>) -> &'c [(&'c F, &'c C)]
    where
        Self: 'a,
    {
        store
    }
}
impl<R, Pt: 'static, X, Rt: 'static, F: 'static> Run<R, Pt, X> for NodeSet<Rt, F>
where
    R: TabAt<Pt, HNil, Rt, AtFk = F>,
{
    type Ex<'a> = ();
    type Store<'b> = Vec<&'b F>;
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X) -> Self::Ex<'a>
    where
        Self: 'a,
    {
    }
    fn gather<'a: 'b, 'b>(tab: &'b R::Table<'a>) -> Vec<&'b F>
    where
        Self: 'a,
    {
        R::rows(tab).iter().map(|(f, _)| f).collect()
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(_: (), store: &'c Vec<&'b F>) -> &'c [&'c F]
    where
        Self: 'a,
    {
        store
    }
}

/// Handing one member of an `Under`, whose full key is `M`, its part of
/// the handles: the exclusive entries at or below it, taken out, and the
/// shared ones, copied.
pub trait PartPlan<M>: Plan {
    /// The member's handles.
    fn part<'a>(hs: &mut Self::Hs<'a>, member: &M) -> Self::Hs<'a>
    where
        Self: 'a;
}
impl<M> PartPlan<M> for () {
    fn part<'a>(_: &mut (), _: &M)
    where
        Self: 'a,
    {
    }
}
impl<M, A: PartPlan<M>, B: PartPlan<M>> PartPlan<M> for (A, B) {
    fn part<'a>((a, b): &mut Self::Hs<'a>, member: &M) -> Self::Hs<'a>
    where
        Self: 'a,
    {
        (A::part(a, member), B::part(b, member))
    }
}
impl<M, Rt: 'static, J: 'static, F: HasPrefix<M> + 'static, X: 'static> PartPlan<M>
    for ExCol<Rt, J, F, X>
{
    fn part<'a>(hs: &mut Vec<(F, &'a mut X)>, member: &M) -> Vec<(F, &'a mut X)>
    where
        Self: 'a,
    {
        let (mine, rest) = core::mem::take(hs)
            .into_iter()
            .partition(|(f, _)| f.has_prefix(member));
        *hs = rest;
        mine
    }
}
impl<M, Rt: 'static, J: 'static, F: 'static, X: 'static> PartPlan<M> for ShCol<Rt, J, F, X> {
    fn part<'a>(hs: &mut Self::Hs<'a>, _: &M) -> Self::Hs<'a>
    where
        Self: 'a,
    {
        *hs
    }
}
impl<M, Rt: 'static, F: 'static> PartPlan<M> for NodeSet<Rt, F> {
    fn part<'a>(hs: &mut Self::Hs<'a>, _: &M) -> Self::Hs<'a>
    where
        Self: 'a,
    {
        *hs
    }
}

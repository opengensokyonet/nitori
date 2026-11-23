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

use frunk_core::hlist::{HCons, HNil};

use super::{
    context::LendContext,
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
    type Hs<'a> = Shared<'a, F, X>;
}
impl Plan for HNil {
    type Hs<'a> = HNil;
}
impl<A: Plan, B: Plan> Plan for HCons<A, B> {
    type Hs<'a> = HCons<A::Hs<'a>, B::Hs<'a>>;
}

/// The handle of a shared column: the entries gathered from the rows, or
/// those of a column a `Plus` holds exclusive, read through it.
pub enum Shared<'a, F, X> {
    /// Gathered from the rows.
    Rows(&'a [(&'a F, &'a X)]),
    /// Read through a column held exclusive.
    Held(&'a [(F, &'a mut X)]),
}
impl<F, X> Clone for Shared<'_, F, X> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F, X> Copy for Shared<'_, F, X> {}
impl<'a, F, X> Shared<'a, F, X> {
    /// Every entry: a node's full key and its component.
    pub fn iter(self) -> impl Iterator<Item = (&'a F, &'a X)> {
        let (rows, held) = match self {
            Self::Rows(r) => (Some(r), None),
            Self::Held(h) => (None, Some(h)),
        };
        let rows = rows.into_iter().flatten().map(|&(f, x)| (f, x));
        let held = held.into_iter().flatten().map(|(f, x)| (f, &**x));
        rows.chain(held)
    }
}
impl<Rt: 'static, F: 'static> Plan for NodeSet<Rt, F> {
    type Hs<'a> = &'a [&'a F];
}

/// Taking a plan from the tables of the root kind `R` for the parts `Pt`,
/// and from the row of the context `X` of the lending.
///
/// The tables borrow the tree for `'a`; the second pass borrows the tables
/// for `'b`, and the handles borrow what it gathered for `'c`.
pub trait Run<R: KindTab<Pt, HNil>, Pt: 'static, X: LendContext>: Plan {
    /// What the first pass takes.
    type Ex<'a>;
    /// What the second pass gathers.
    type Store<'b>;
    /// The first pass: the exclusive columns, taken out of the rows, and
    /// the pieces of the context, out of its row.
    fn take<'a>(tab: &mut R::Table<'a>, cx: &mut X::Row<'a>) -> Self::Ex<'a>
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
impl<R: KindTab<Pt, HNil>, Pt: 'static, X: LendContext> Run<R, Pt, X> for () {
    type Ex<'a> = ();
    type Store<'b> = ();
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X::Row<'a>) -> Self::Ex<'a>
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
impl<R: KindTab<Pt, HNil>, Pt: 'static, X: LendContext> Run<R, Pt, X> for HNil {
    type Ex<'a> = HNil;
    type Store<'b> = HNil;
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X::Row<'a>) -> HNil
    where
        Self: 'a,
    {
        HNil
    }
    fn gather<'a: 'b, 'b>(_: &'b R::Table<'a>) -> HNil
    where
        Self: 'a,
    {
        HNil
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(_: HNil, _: &'c HNil) -> HNil
    where
        Self: 'a,
    {
        HNil
    }
}
impl<R: KindTab<Pt, HNil>, Pt: 'static, X: LendContext, A, B> Run<R, Pt, X> for HCons<A, B>
where
    A: Run<R, Pt, X>,
    B: Run<R, Pt, X>,
{
    type Ex<'a> = HCons<A::Ex<'a>, B::Ex<'a>>;
    type Store<'b> = HCons<A::Store<'b>, B::Store<'b>>;
    fn take<'a>(tab: &mut R::Table<'a>, cx: &mut X::Row<'a>) -> Self::Ex<'a>
    where
        Self: 'a,
    {
        let head = A::take(tab, cx);
        HCons {
            head,
            tail: B::take(tab, cx),
        }
    }
    fn gather<'a: 'b, 'b>(tab: &'b R::Table<'a>) -> Self::Store<'b>
    where
        Self: 'a,
    {
        HCons {
            head: A::gather(tab),
            tail: B::gather(tab),
        }
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(ex: Self::Ex<'a>, store: &'c Self::Store<'b>) -> Self::Hs<'c>
    where
        Self: 'a,
    {
        HCons {
            head: A::handles(ex.head, &store.head),
            tail: B::handles(ex.tail, &store.tail),
        }
    }
}
impl<R: KindTab<Pt, HNil>, Pt: 'static, X: LendContext, A, B> Run<R, Pt, X> for (A, B)
where
    A: Run<R, Pt, X>,
    B: Run<R, Pt, X>,
{
    type Ex<'a> = (A::Ex<'a>, B::Ex<'a>);
    type Store<'b> = (A::Store<'b>, B::Store<'b>);
    fn take<'a>(tab: &mut R::Table<'a>, cx: &mut X::Row<'a>) -> Self::Ex<'a>
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
impl<R, Pt: 'static, X: LendContext, Rt: 'static, J: 'static, F: Clone + 'static, C: 'static>
    Run<R, Pt, X> for ExCol<Rt, J, F, C>
where
    R: TabAt<Pt, HNil, Rt, AtFk = F>,
    R::AtComps: SlotAt<J, C = C>,
{
    type Ex<'a> = Vec<(F, &'a mut C)>;
    type Store<'b> = ();
    fn take<'a>(tab: &mut R::Table<'a>, _: &mut X::Row<'a>) -> Vec<(F, &'a mut C)>
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
impl<R, Pt: 'static, X: LendContext, Rt: 'static, J: 'static, F: 'static, C: 'static> Run<R, Pt, X>
    for ShCol<Rt, J, F, C>
where
    R: TabAt<Pt, HNil, Rt, AtFk = F>,
    R::AtComps: SlotAt<J, C = C>,
{
    type Ex<'a> = ();
    type Store<'b> = Vec<(&'b F, &'b C)>;
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X::Row<'a>) -> Self::Ex<'a>
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
    fn handles<'a: 'c, 'b: 'c, 'c>(_: (), store: &'c Vec<(&'b F, &'b C)>) -> Shared<'c, F, C>
    where
        Self: 'a,
    {
        Shared::Rows(store)
    }
}
impl<R, Pt: 'static, X: LendContext, Rt: 'static, F: 'static> Run<R, Pt, X> for NodeSet<Rt, F>
where
    R: TabAt<Pt, HNil, Rt, AtFk = F>,
{
    type Ex<'a> = ();
    type Store<'b> = Vec<&'b F>;
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X::Row<'a>) -> Self::Ex<'a>
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
impl<M> PartPlan<M> for HNil {
    fn part<'a>(_: &mut HNil, _: &M) -> HNil
    where
        Self: 'a,
    {
        HNil
    }
}
impl<M, A: PartPlan<M>, B: PartPlan<M>> PartPlan<M> for HCons<A, B> {
    fn part<'a>(hs: &mut Self::Hs<'a>, member: &M) -> Self::Hs<'a>
    where
        Self: 'a,
    {
        HCons {
            head: A::part(&mut hs.head, member),
            tail: B::part(&mut hs.tail, member),
        }
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

/// The handles of a plan lent again, for a shorter time: what a view that
/// hands out one member at a time gives each of them.
pub trait Rebor: Plan {
    /// The handles, borrowed from `hs` for `'b`.
    fn rebor<'b, 'c: 'b>(hs: &'b mut Self::Hs<'c>) -> Self::Hs<'b>;
}
impl Rebor for () {
    fn rebor<'b, 'c: 'b>(_: &'b mut ()) {}
}
impl<A: Rebor, B: Rebor> Rebor for (A, B) {
    fn rebor<'b, 'c: 'b>((a, b): &'b mut Self::Hs<'c>) -> Self::Hs<'b> {
        (A::rebor(a), B::rebor(b))
    }
}
impl Rebor for HNil {
    fn rebor<'b, 'c: 'b>(_: &'b mut HNil) -> HNil {
        HNil
    }
}
impl<A: Rebor, B: Rebor> Rebor for HCons<A, B> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut Self::Hs<'c>) -> Self::Hs<'b> {
        HCons {
            head: A::rebor(&mut hs.head),
            tail: B::rebor(&mut hs.tail),
        }
    }
}
impl<Rt: 'static, J: 'static, F: Clone + 'static, X: 'static> Rebor for ExCol<Rt, J, F, X> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut Vec<(F, &'c mut X)>) -> Vec<(F, &'b mut X)> {
        hs.iter_mut().map(|(f, x)| (f.clone(), &mut **x)).collect()
    }
}
impl<Rt: 'static, J: 'static, F: 'static, X: 'static> Rebor for ShCol<Rt, J, F, X> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut Shared<'c, F, X>) -> Shared<'b, F, X> {
        *hs
    }
}
impl<Rt: 'static, F: 'static> Rebor for NodeSet<Rt, F> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut &'c [&'c F]) -> &'b [&'b F] {
        hs
    }
}

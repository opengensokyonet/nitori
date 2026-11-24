//! Column plans: what a list of needs takes from the tables, and the
//! handles it is given.
//!
//! A plan is a tree of pairs whose leaves are an exclusive column
//! ([`ExCol`]), a shared column ([`ShCol`]) or the full keys of one kind's
//! nodes ([`NodeSet`]). Before the walk, every leaf marks the table of its
//! kind, and a node set the anchor it is below ([`Run::mark`]), so that
//! the walk visits only what the plan takes. The plan is then taken in two
//! passes: the exclusive columns first, in the order of the plan, each
//! component taken out of its row; then, with the rows left shared, the
//! shared columns and node sets, any of them more than once. That no two
//! needs take the same column with one of them exclusive is checked on the
//! types where the needs are proved, so a component missing from its row
//! is a broken invariant.
//!
//! Every entry of a column keeps its node's place in the walk, so that the
//! members of an `Under` are each given their entries in one pass
//! ([`PartPlan`]).
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

use super::{
    context::LendContext,
    list::RouteEq,
    rows::{self, Rows, Span},
    table::{KindTab, SlotAt, TabAt},
};
use crate::id::{False, True};

/// The component `X` of every node of the kind at the route `Rt`, at the
/// position `J` of its components, exclusive; the nodes have full keys
/// `F`.
pub struct ExCol<Rt, J, F, X>(PhantomData<fn() -> (Rt, J, F, X)>);
/// The component `X` of every node of the kind at `Rt`, shared.
pub struct ShCol<Rt, J, F, X>(PhantomData<fn() -> (Rt, J, F, X)>);
/// The full keys of every node of the kind at `Rt` below the anchor,
/// the node of the kind at `A` on the route to the lend point.
pub struct NodeSet<Rt, F, A>(PhantomData<fn() -> (Rt, F, A)>);

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
    type Hs<'a> = Rows<(F, &'a mut X, Span)>;
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
    Rows(&'a [(&'a F, &'a X, Span)]),
    /// Read through a column held exclusive.
    Held(&'a [(F, &'a mut X, Span)]),
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
        let rows = rows.into_iter().flatten().map(|&(f, x, _)| (f, x));
        let held = held.into_iter().flatten().map(|(f, x, _)| (f, &**x));
        rows.chain(held)
    }
    /// The entries of the member at `member`, the members coming in the
    /// order of the walk from `from` ([`rows::member_rows`]).
    fn part(self, from: &mut usize, member: Span) -> Self {
        match self {
            Self::Rows(r) => Self::Rows(&r[rows::member_rows(|i| r[i].2, r.len(), from, member)]),
            Self::Held(h) => Self::Held(&h[rows::member_rows(|i| h[i].2, h.len(), from, member)]),
        }
    }
    /// The entries of the member at `member` alone.
    fn part_of(self, member: Span) -> Self {
        match self {
            Self::Rows(r) => Self::Rows(&r[rows::member_rows_of(r, |e| e.2, member)]),
            Self::Held(h) => Self::Held(&h[rows::member_rows_of(h, |e| e.2, member)]),
        }
    }
}
impl<Rt: 'static, F: 'static, A: 'static> Plan for NodeSet<Rt, F, A> {
    type Hs<'a> = &'a [(&'a F, Span)];
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
    /// Mark, in the tables, what the plan takes, before the walk.
    fn mark(tab: &mut R::Table<'_>);
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
    fn mark(_: &mut R::Table<'_>) {}
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
    fn mark(_: &mut R::Table<'_>) {}
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
    fn mark(tab: &mut R::Table<'_>) {
        A::mark(tab);
        B::mark(tab);
    }
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
    fn mark(tab: &mut R::Table<'_>) {
        A::mark(tab);
        B::mark(tab);
    }
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
    type Ex<'a> = Rows<(F, &'a mut C, Span)>;
    type Store<'b> = ();
    fn mark(tab: &mut R::Table<'_>) {
        R::want_mut(tab).rows = true;
    }
    fn take<'a>(tab: &mut R::Table<'a>, _: &mut X::Row<'a>) -> Rows<(F, &'a mut C, Span)>
    where
        Self: 'a,
    {
        R::rows_mut(tab)
            .iter_mut()
            .map(|r| {
                // Invariant: the needs were checked not to overlap, so no
                // need before this one took the component.
                let x = <R::AtComps as SlotAt<J>>::slot_mut(&mut r.row)
                    .take()
                    .expect("an exclusive column is taken once in a lending");
                (r.fk.clone(), x, r.span)
            })
            .collect()
    }
    fn gather<'a: 'b, 'b>(_: &'b R::Table<'a>) -> Self::Store<'b>
    where
        Self: 'a,
    {
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(
        ex: Rows<(F, &'a mut C, Span)>,
        _: &'c (),
    ) -> Rows<(F, &'c mut C, Span)>
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
    type Store<'b> = Rows<(&'b F, &'b C, Span)>;
    fn mark(tab: &mut R::Table<'_>) {
        R::want_mut(tab).rows = true;
    }
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X::Row<'a>) -> Self::Ex<'a>
    where
        Self: 'a,
    {
    }
    fn gather<'a: 'b, 'b>(tab: &'b R::Table<'a>) -> Rows<(&'b F, &'b C, Span)>
    where
        Self: 'a,
    {
        R::rows(tab)
            .iter()
            .map(|r| {
                // Invariant: no exclusive need took a column a shared need
                // takes, which the check of overlaps refuses.
                let x = <R::AtComps as SlotAt<J>>::slot(&r.row)
                    .as_deref()
                    .expect("a shared column is not taken exclusive in the same lending");
                (&r.fk, x, r.span)
            })
            .collect()
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(_: (), store: &'c Rows<(&'b F, &'b C, Span)>) -> Shared<'c, F, C>
    where
        Self: 'a,
    {
        Shared::Rows(store)
    }
}
impl<R, Pt: 'static, X: LendContext, Rt: 'static, F: 'static, A: 'static> Run<R, Pt, X>
    for NodeSet<Rt, F, A>
where
    R: TabAt<Pt, HNil, Rt, AtFk = F> + TabAt<Pt, HNil, A>,
{
    type Ex<'a> = ();
    type Store<'b> = Rows<(&'b F, Span)>;
    fn mark(tab: &mut R::Table<'_>) {
        <R as TabAt<Pt, HNil, Rt>>::want_mut(tab).rows = true;
        <R as TabAt<Pt, HNil, A>>::want_mut(tab).anchor = true;
    }
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X::Row<'a>) -> Self::Ex<'a>
    where
        Self: 'a,
    {
    }
    fn gather<'a: 'b, 'b>(tab: &'b R::Table<'a>) -> Rows<(&'b F, Span)>
    where
        Self: 'a,
    {
        <R as TabAt<Pt, HNil, Rt>>::rows(tab)
            .iter()
            .map(|r| (&r.fk, r.span))
            .collect()
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(_: (), store: &'c Rows<(&'b F, Span)>) -> &'c [(&'c F, Span)]
    where
        Self: 'a,
    {
        store
    }
}

/// Handing the members of an `Under`, of the kind at the route `MRt`,
/// their parts of the handles, the members coming in the order of the
/// walk: the exclusive entries at or below each, taken out, and the shared
/// ones above or below it, copied; what is below an anchor above the
/// members ([`Anchored`]), copied whole.
pub trait PartPlan<MRt>: Plan {
    /// What is left to hand out.
    type Left<'a>
    where
        Self: 'a;
    /// All of the handles, to hand out.
    fn left<'a>(hs: Self::Hs<'a>) -> Self::Left<'a>
    where
        Self: 'a;
    /// The part of the member at `member`, after the members before it.
    fn part<'a>(left: &mut Self::Left<'a>, member: Span) -> Self::Hs<'a>
    where
        Self: 'a;
}
impl<MRt> PartPlan<MRt> for () {
    type Left<'a> = ();
    fn left<'a>(_: ())
    where
        Self: 'a,
    {
    }
    fn part<'a>(_: &mut (), _: Span)
    where
        Self: 'a,
    {
    }
}
impl<MRt> PartPlan<MRt> for HNil {
    type Left<'a> = HNil;
    fn left<'a>(_: HNil) -> HNil
    where
        Self: 'a,
    {
        HNil
    }
    fn part<'a>(_: &mut HNil, _: Span) -> HNil
    where
        Self: 'a,
    {
        HNil
    }
}
impl<MRt, A: PartPlan<MRt>, B: PartPlan<MRt>> PartPlan<MRt> for HCons<A, B> {
    type Left<'a>
        = HCons<A::Left<'a>, B::Left<'a>>
    where
        Self: 'a;
    fn left<'a>(hs: Self::Hs<'a>) -> Self::Left<'a>
    where
        Self: 'a,
    {
        HCons {
            head: A::left(hs.head),
            tail: B::left(hs.tail),
        }
    }
    fn part<'a>(left: &mut Self::Left<'a>, member: Span) -> Self::Hs<'a>
    where
        Self: 'a,
    {
        HCons {
            head: A::part(&mut left.head, member),
            tail: B::part(&mut left.tail, member),
        }
    }
}
impl<MRt, A: PartPlan<MRt>, B: PartPlan<MRt>> PartPlan<MRt> for (A, B) {
    type Left<'a>
        = (A::Left<'a>, B::Left<'a>)
    where
        Self: 'a;
    fn left<'a>((a, b): Self::Hs<'a>) -> Self::Left<'a>
    where
        Self: 'a,
    {
        (A::left(a), B::left(b))
    }
    fn part<'a>((a, b): &mut Self::Left<'a>, member: Span) -> Self::Hs<'a>
    where
        Self: 'a,
    {
        (A::part(a, member), B::part(b, member))
    }
}
/// The entries of an exclusive column are at or below the members, the
/// check refusing one above them: each member's are the next in the walk.
impl<MRt, Rt: 'static, J: 'static, F: 'static, X: 'static> PartPlan<MRt> for ExCol<Rt, J, F, X> {
    type Left<'a> = core::iter::Peekable<rows::IntoIter<(F, &'a mut X, Span)>>;
    fn left<'a>(hs: Rows<(F, &'a mut X, Span)>) -> Self::Left<'a>
    where
        Self: 'a,
    {
        hs.into_iter().peekable()
    }
    fn part<'a>(left: &mut Self::Left<'a>, member: Span) -> Rows<(F, &'a mut X, Span)>
    where
        Self: 'a,
    {
        while left.next_if(|e| e.2.end <= member.start).is_some() {}
        core::iter::from_fn(|| left.next_if(|e| e.2.start < member.end)).collect()
    }
}
impl<MRt, Rt: 'static, J: 'static, F: 'static, X: 'static> PartPlan<MRt> for ShCol<Rt, J, F, X> {
    type Left<'a> = (Shared<'a, F, X>, usize);
    fn left<'a>(hs: Shared<'a, F, X>) -> Self::Left<'a>
    where
        Self: 'a,
    {
        (hs, 0)
    }
    fn part<'a>((hs, from): &mut Self::Left<'a>, member: Span) -> Shared<'a, F, X>
    where
        Self: 'a,
    {
        hs.part(from, member)
    }
}
impl<MRt, Rt: 'static, F: 'static, A: 'static> PartPlan<MRt> for NodeSet<Rt, F, A> {
    type Left<'a> = (&'a [(&'a F, Span)], usize);
    fn left<'a>(hs: Self::Hs<'a>) -> Self::Left<'a>
    where
        Self: 'a,
    {
        (hs, 0)
    }
    fn part<'a>((hs, from): &mut Self::Left<'a>, member: Span) -> Self::Hs<'a>
    where
        Self: 'a,
    {
        &hs[rows::member_rows(|i| hs[i].1, hs.len(), from, member)]
    }
}

/// The part of one member's handles, of the kind at the route `MRt`, lent
/// again for a shorter time: what a view that hands out one member at a
/// time gives each of them.
pub trait Rebor<MRt>: Plan {
    /// The member's part of `hs`, the member being at `member`, borrowed
    /// for `'b`.
    fn rebor<'b, 'c: 'b>(hs: &'b mut Self::Hs<'c>, member: Span) -> Self::Hs<'b>;
}
impl<MRt> Rebor<MRt> for () {
    fn rebor<'b, 'c: 'b>(_: &'b mut (), _: Span) {}
}
impl<MRt, A: Rebor<MRt>, B: Rebor<MRt>> Rebor<MRt> for (A, B) {
    fn rebor<'b, 'c: 'b>((a, b): &'b mut Self::Hs<'c>, member: Span) -> Self::Hs<'b> {
        (A::rebor(a, member), B::rebor(b, member))
    }
}
impl<MRt> Rebor<MRt> for HNil {
    fn rebor<'b, 'c: 'b>(_: &'b mut HNil, _: Span) -> HNil {
        HNil
    }
}
impl<MRt, A: Rebor<MRt>, B: Rebor<MRt>> Rebor<MRt> for HCons<A, B> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut Self::Hs<'c>, member: Span) -> Self::Hs<'b> {
        HCons {
            head: A::rebor(&mut hs.head, member),
            tail: B::rebor(&mut hs.tail, member),
        }
    }
}
impl<MRt, Rt: 'static, J: 'static, F: Clone + 'static, X: 'static> Rebor<MRt>
    for ExCol<Rt, J, F, X>
{
    fn rebor<'b, 'c: 'b>(
        hs: &'b mut Rows<(F, &'c mut X, Span)>,
        member: Span,
    ) -> Rows<(F, &'b mut X, Span)> {
        let part = rows::member_rows_of(hs, |e| e.2, member);
        hs[part]
            .iter_mut()
            .map(|(f, x, s)| (f.clone(), &mut **x, *s))
            .collect()
    }
}
impl<MRt, Rt: 'static, J: 'static, F: 'static, X: 'static> Rebor<MRt> for ShCol<Rt, J, F, X> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut Shared<'c, F, X>, member: Span) -> Shared<'b, F, X> {
        hs.part_of(member)
    }
}
impl<MRt, Rt: 'static, F: 'static, A: 'static> Rebor<MRt> for NodeSet<Rt, F, A> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut &'c [(&'c F, Span)], member: Span) -> &'b [(&'b F, Span)] {
        &hs[rows::member_rows_of(hs, |e| e.1, member)]
    }
}

// ------------------------------------------------------------------ anchors

/// The plan `P` of an `Under` or of the candidates of a locator, whose
/// anchor is the node of the kind at the route `A`: inside the members of
/// an `Under`, its entries are each member's only when it is anchored at
/// the member; anchored above it, they are every member's.
pub struct Anchored<A, P>(PhantomData<fn() -> (A, P)>);
impl<A: 'static, P: Plan> Plan for Anchored<A, P> {
    type Hs<'a> = P::Hs<'a>;
}
impl<R: KindTab<Pt, HNil>, Pt: 'static, X: LendContext, A: 'static, P: Run<R, Pt, X>> Run<R, Pt, X>
    for Anchored<A, P>
{
    type Ex<'a> = P::Ex<'a>;
    type Store<'b> = P::Store<'b>;
    fn mark(tab: &mut R::Table<'_>) {
        P::mark(tab);
    }
    fn take<'a>(tab: &mut R::Table<'a>, cx: &mut X::Row<'a>) -> Self::Ex<'a>
    where
        Self: 'a,
    {
        P::take(tab, cx)
    }
    fn gather<'a: 'b, 'b>(tab: &'b R::Table<'a>) -> Self::Store<'b>
    where
        Self: 'a,
    {
        P::gather(tab)
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(ex: Self::Ex<'a>, store: &'c Self::Store<'b>) -> Self::Hs<'c>
    where
        Self: 'a,
    {
        P::handles(ex, store)
    }
}
impl<MRt, A: RouteEq<MRt> + 'static, P: Plan> PartPlan<MRt> for Anchored<A, P>
where
    A::Out: PartBy<P, MRt>,
{
    type Left<'a>
        = <A::Out as PartBy<P, MRt>>::Left<'a>
    where
        Self: 'a;
    fn left<'a>(hs: P::Hs<'a>) -> Self::Left<'a>
    where
        Self: 'a,
    {
        <A::Out as PartBy<P, MRt>>::left(hs)
    }
    fn part<'a>(left: &mut Self::Left<'a>, member: Span) -> P::Hs<'a>
    where
        Self: 'a,
    {
        <A::Out as PartBy<P, MRt>>::part(left, member)
    }
}
impl<MRt, A: RouteEq<MRt> + 'static, P: Plan> Rebor<MRt> for Anchored<A, P>
where
    A::Out: PartBy<P, MRt>,
{
    fn rebor<'b, 'c: 'b>(hs: &'b mut P::Hs<'c>, member: Span) -> P::Hs<'b> {
        <A::Out as PartBy<P, MRt>>::rebor(hs, member)
    }
}
impl<A: 'static, P: Copied> Copied for Anchored<A, P> {
    fn copied<'b, 'c: 'b>(hs: &'b Self::Hs<'c>) -> Self::Hs<'c> {
        P::copied(hs)
    }
    fn shortened<'b, 'c: 'b>(hs: &'b Self::Hs<'c>) -> Self::Hs<'b> {
        P::shortened(hs)
    }
}

/// Handing out the plan `P` of an `Under` anchored at the members
/// ([`True`]), by member, or above them ([`False`]), whole.
pub trait PartBy<P: Plan, MRt> {
    /// What is left to hand out.
    type Left<'a>
    where
        P: 'a;
    /// All of the handles, to hand out.
    fn left<'a>(hs: P::Hs<'a>) -> Self::Left<'a>;
    /// The part of the member at `member`.
    fn part<'a>(left: &mut Self::Left<'a>, member: Span) -> P::Hs<'a>;
    /// The part of the member at `member`, lent again.
    fn rebor<'b, 'c: 'b>(hs: &'b mut P::Hs<'c>, member: Span) -> P::Hs<'b>;
}
impl<P: PartPlan<MRt> + Rebor<MRt>, MRt> PartBy<P, MRt> for True {
    type Left<'a>
        = P::Left<'a>
    where
        P: 'a;
    fn left<'a>(hs: P::Hs<'a>) -> P::Left<'a> {
        P::left(hs)
    }
    fn part<'a>(left: &mut P::Left<'a>, member: Span) -> P::Hs<'a> {
        P::part(left, member)
    }
    fn rebor<'b, 'c: 'b>(hs: &'b mut P::Hs<'c>, member: Span) -> P::Hs<'b> {
        P::rebor(hs, member)
    }
}
/// Above the members, an `Under` only reads, its check refusing what is
/// taken exclusive there: every member reads all of it.
impl<P: Copied, MRt> PartBy<P, MRt> for False {
    type Left<'a>
        = P::Hs<'a>
    where
        P: 'a;
    fn left<'a>(hs: P::Hs<'a>) -> P::Hs<'a> {
        hs
    }
    fn part<'a>(left: &mut P::Hs<'a>, _: Span) -> P::Hs<'a> {
        P::copied(left)
    }
    fn rebor<'b, 'c: 'b>(hs: &'b mut P::Hs<'c>, _: Span) -> P::Hs<'b> {
        P::shortened(hs)
    }
}

/// A plan that only reads: its handles copied.
pub trait Copied: Plan {
    /// The handles, copied.
    fn copied<'b, 'c: 'b>(hs: &'b Self::Hs<'c>) -> Self::Hs<'c>;
    /// The handles, copied for a shorter time.
    fn shortened<'b, 'c: 'b>(hs: &'b Self::Hs<'c>) -> Self::Hs<'b>;
}
impl Copied for () {
    fn copied<'b, 'c: 'b>(_: &'b ()) {}
    fn shortened<'b, 'c: 'b>(_: &'b ()) {}
}
impl Copied for HNil {
    fn copied<'b, 'c: 'b>(_: &'b HNil) -> HNil {
        HNil
    }
    fn shortened<'b, 'c: 'b>(_: &'b HNil) -> HNil {
        HNil
    }
}
impl<A: Copied, B: Copied> Copied for (A, B) {
    fn copied<'b, 'c: 'b>((a, b): &'b Self::Hs<'c>) -> Self::Hs<'c> {
        (A::copied(a), B::copied(b))
    }
    fn shortened<'b, 'c: 'b>((a, b): &'b Self::Hs<'c>) -> Self::Hs<'b> {
        (A::shortened(a), B::shortened(b))
    }
}
impl<A: Copied, B: Copied> Copied for HCons<A, B> {
    fn copied<'b, 'c: 'b>(hs: &'b Self::Hs<'c>) -> Self::Hs<'c> {
        HCons {
            head: A::copied(&hs.head),
            tail: B::copied(&hs.tail),
        }
    }
    fn shortened<'b, 'c: 'b>(hs: &'b Self::Hs<'c>) -> Self::Hs<'b> {
        HCons {
            head: A::shortened(&hs.head),
            tail: B::shortened(&hs.tail),
        }
    }
}
impl<Rt: 'static, J: 'static, F: 'static, X: 'static> Copied for ShCol<Rt, J, F, X> {
    fn copied<'b, 'c: 'b>(hs: &'b Shared<'c, F, X>) -> Shared<'c, F, X> {
        *hs
    }
    fn shortened<'b, 'c: 'b>(hs: &'b Shared<'c, F, X>) -> Shared<'b, F, X> {
        *hs
    }
}
impl<Rt: 'static, F: 'static, A: 'static> Copied for NodeSet<Rt, F, A> {
    fn copied<'b, 'c: 'b>(hs: &'b &'c [(&'c F, Span)]) -> &'c [(&'c F, Span)] {
        hs
    }
    fn shortened<'b, 'c: 'b>(hs: &'b &'c [(&'c F, Span)]) -> &'b [(&'b F, Span)] {
        hs
    }
}

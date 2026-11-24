//! The identities of the nodes of one kind: which node each row is, what
//! a component's waker names it by.
//!
//! A column of identities ([`IdentCol`]) is read like the full keys of a
//! kind's nodes ([`NodeSet`]): shared, copied to every member of an
//! `Under` and to every branch of a `+`, overlapping nothing.
use core::marker::PhantomData;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::{
    context::{CxCol, LendContext},
    list::RouteEq,
    plan::{Copied, ExCol, NodeSet, PartPlan, Plan, Rebor, Run, ShCol},
    plus::{IndexOf, Item, Merge, Norm, Pick, SameItem, Union},
    rows::{self, Rows, Span},
    table::TabAt,
};
use crate::{id::False, structure::NodeIdent};

/// Which node every node of the kind at the route `Rt` is, with its full
/// key `F`.
pub struct IdentCol<Rt, F>(PhantomData<fn() -> (Rt, F)>);

/// The identities of a kind's nodes, each with its full key and its place
/// in the walk.
pub type Idents<'a, F> = &'a [(&'a F, NodeIdent, Span)];

impl<Rt: 'static, F: 'static> Plan for IdentCol<Rt, F> {
    type Hs<'a> = Idents<'a, F>;
}
impl<R, Pt: 'static, X: LendContext, Rt: 'static, F: 'static> Run<R, Pt, X> for IdentCol<Rt, F>
where
    R: TabAt<Pt, HNil, Rt, AtFk = F>,
{
    type Ex<'a> = ();
    type Store<'b> = Rows<(&'b F, NodeIdent, Span)>;
    fn mark(tab: &mut R::Table<'_>) {
        R::want_mut(tab).rows = true;
    }
    fn take<'a>(_: &mut R::Table<'a>, _: &mut X::Row<'a>) -> Self::Ex<'a>
    where
        Self: 'a,
    {
    }
    fn gather<'a: 'b, 'b>(tab: &'b R::Table<'a>) -> Rows<(&'b F, NodeIdent, Span)>
    where
        Self: 'a,
    {
        R::rows(tab)
            .iter()
            .map(|r| (&r.fk, r.ident, r.span))
            .collect()
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(
        _: (),
        store: &'c Rows<(&'b F, NodeIdent, Span)>,
    ) -> Idents<'c, F>
    where
        Self: 'a,
    {
        store
    }
}
impl<MRt, Rt: 'static, F: 'static> PartPlan<MRt> for IdentCol<Rt, F> {
    type Left<'a> = (Idents<'a, F>, usize);
    fn left<'a>(hs: Idents<'a, F>) -> Self::Left<'a>
    where
        Self: 'a,
    {
        (hs, 0)
    }
    fn part<'a>((hs, from): &mut Self::Left<'a>, member: Span) -> Idents<'a, F>
    where
        Self: 'a,
    {
        &hs[rows::member_rows(|i| hs[i].2, hs.len(), from, member)]
    }
}
impl<MRt, Rt: 'static, F: 'static> Rebor<MRt> for IdentCol<Rt, F> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut Idents<'c, F>, member: Span) -> Idents<'b, F> {
        &hs[rows::member_rows_of(hs, |e| e.2, member)]
    }
}
impl<Rt: 'static, F: 'static> Copied for IdentCol<Rt, F> {
    fn copied<'b, 'c: 'b>(hs: &'b Idents<'c, F>) -> Idents<'c, F> {
        hs
    }
    fn shortened<'b, 'c: 'b>(hs: &'b Idents<'c, F>) -> Idents<'b, F> {
        hs
    }
}

// ------------------------------------------------------------------ in a `Plus`

impl<Rt, F> Norm for IdentCol<Rt, F> {
    type Out = HCons<IdentCol<Rt, F>, HNil>;
}
impl<R1: RouteEq<R2>, F1, R2, F2> SameItem<IdentCol<R2, F2>> for IdentCol<R1, F1> {
    type Out = R1::Out;
}
macro_rules! differs {
    ($([$($g:ident),*] $other:ty;)*) => {$(
        impl<R1, F1, $($g),*> SameItem<$other> for IdentCol<R1, F1> {
            type Out = False;
        }
        impl<R1, F1, $($g),*> SameItem<IdentCol<R1, F1>> for $other {
            type Out = False;
        }
    )*};
}
differs! {
    [R2, J2, F2, X2] ExCol<R2, J2, F2, X2>;
    [R2, J2, F2, X2] ShCol<R2, J2, F2, X2>;
    [R2, F2, A2] NodeSet<R2, F2, A2>;
    [I2, P2] CxCol<I2, P2>;
}
impl<Rt, F, B> Merge<B> for IdentCol<Rt, F> {
    type Out = Self;
}
impl<Rt: 'static, F: 'static> Item for IdentCol<Rt, F> {
    type Slot<'b, 'c>
        = &'b [(&'c F, NodeIdent, Span)]
    where
        'c: 'b;
    type Seen<'b, 'c>
        = &'b [(&'c F, NodeIdent, Span)]
    where
        'c: 'b;
    fn slot<'b, 'c: 'b>(hs: &'b mut Idents<'c, F>) -> &'b [(&'c F, NodeIdent, Span)] {
        hs
    }
    fn seen<'b, 'c: 'b>(slot: &'b [(&'c F, NodeIdent, Span)]) -> &'b [(&'c F, NodeIdent, Span)] {
        slot
    }
}

/// The identities of the nodes of one kind, with full keys `F`, are in
/// the union at `I`.
pub trait IdentsAt<I>: Union {
    /// The nodes' full keys.
    type F: 'static;
    /// The identities.
    fn idents<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [(&'c Self::F, NodeIdent, Span)];
}
impl<Rt: 'static, F: 'static, T: Union> IdentsAt<Here> for HCons<IdentCol<Rt, F>, T> {
    type F = F;
    fn idents<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [(&'c F, NodeIdent, Span)] {
        seen.head
    }
}
impl<H: Item, T: IdentsAt<I>, I> IdentsAt<There<I>> for HCons<H, T> {
    type F = T::F;
    fn idents<'b, 'c: 'b>(seen: Self::Seen<'b, 'c>) -> &'b [(&'c T::F, NodeIdent, Span)] {
        T::idents(seen.tail)
    }
}
impl<U, Rt: 'static, F: 'static> Pick<U> for IdentCol<Rt, F>
where
    U: IndexOf<IdentCol<Rt, F>> + IdentsAt<<U as IndexOf<IdentCol<Rt, F>>>::I, F = F>,
{
    type Ex<'b> = ();
    fn ex<'b, 'c: 'b>(_: &mut U::Slots<'b, 'c>) {}
    fn pick<'b, 'c: 'b>(_: (), seen: U::Seen<'b, 'c>) -> Idents<'b, F> {
        <U as IdentsAt<_>>::idents(seen)
    }
}

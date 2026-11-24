//! Lending: a list of needs taken from the tree around a lend point and
//! handed over as its view.
//!
//! A lend point is a kind of node (its place in the structure, as types)
//! and a node's full key (the keys from the root down to it). A lending
//! walks the tree once into one table of rows per kind ([`table`]), takes
//! what each need's plan names in two passes ([`plan`]), and makes the
//! view of each need from its handles ([`Lend`]). Where the needs are
//! proved, their checks ([`check`]) give the reasons a list does not hold.
// The engine computes with types; a marker over several of them is what
// it is made of.
#![allow(clippy::type_complexity)]
use crate::{
    need::{Need, Pos},
    structure::{KindFor, Node, NodeKind},
};

pub(crate) mod build;
pub(crate) mod cap;
pub(crate) mod check;
pub(crate) mod context;
pub(crate) mod cycle;
pub(crate) mod entry;
pub(crate) mod input;
pub(crate) mod list;
pub(crate) mod locator;
pub(crate) mod members;
pub(crate) mod one_at_a_time;
pub(crate) mod own;
pub(crate) mod plan;
pub(crate) mod plus;
pub(crate) mod react;
pub(crate) mod table;
pub(crate) mod this;
pub(crate) mod tree;
pub(crate) mod under;
pub(crate) mod up;
pub(crate) mod waker;

#[cfg(test)]
pub(crate) mod tests;

use check::{Check, DeepEnoughAll, Disjoint, Takes};
pub(crate) use context::LendContext;
use cycle::NoCycle;
use plan::{Plan, Run};
use table::{KindTab, TabAt};
use tree::{Ctx, Cx, Locate};

/// The proof that the need `Self` can be lent at the lend point `C`, at
/// the position `P` (inferred): what it takes and how its view is made.
pub trait Lend<C: Ctx, P: Pos>: Need {
    /// What the need takes.
    type Cols: Plan;
    /// The view, from the handles of the plan; `at` is the lend point's
    /// full key.
    fn build<'a>(hs: <Self::Cols as Plan>::Hs<'a>, at: &C::Fk) -> Self::View<'a, P>
    where
        Self: 'a;
}

/// The lend point of the kind `L` in the tree whose root is `R`, for the
/// parts `Pt`, lent with the context `Xc`; `IL` is where `L` is in the
/// structure (inferred).
pub type PointCx<R, Pt, L, IL, Xc> = Cx<R, Pt, <R as Locate<L, IL>>::Route, list::Z, Xc>;
/// The full key of a node of the kind `L`.
pub type PointFk<R, Pt, L, IL, Xc> = <PointCx<R, Pt, L, IL, Xc> as Ctx>::Fk;

/// Code that takes a view at a position it is generic over, as a call
/// does: the driver gives it the position it inferred.
pub trait Visit<N: Need> {
    /// What it gives back.
    type Out;
    /// Use the view of `N` at the position `P`.
    fn visit<'v, P: Pos>(self, view: N::View<'v, P>) -> Self::Out;
}

/// Lend the needs `N` at the node of the kind `L` with the full key `at`
/// to `f`, with the context `cx` of the lending; `None` when no node has
/// that full key.
///
/// The position `P` is inferred, or given by the caller that proved it
/// elsewhere.
pub(crate) fn lend<R, Pt: 'static, L, IL, N, P, X, F, O>(
    root: &mut Node<R, Pt>,
    at: &PointFk<R, Pt, L, IL, X>,
    cx: &mut X,
    f: F,
) -> Option<O>
where
    R: KindFor<Pt> + NodeKind + Locate<L, IL>,
    PointCx<R, Pt, L, IL, X>: Ctx,
    N: NoCycle<R, Pt, frunk_core::hlist::HNil>,
    N: Check<PointCx<R, Pt, L, IL, X>, P>,
    N: Takes<PointCx<R, Pt, L, IL, X>, P>,
    <N as Takes<PointCx<R, Pt, L, IL, X>, P>>::Touches: Disjoint + DeepEnoughAll,
    N: Lend<PointCx<R, Pt, L, IL, X>, P>,
    P: Pos,
    R: TabAt<
            Pt,
            frunk_core::hlist::HNil,
            <R as Locate<L, IL>>::Route,
            AtFk = PointFk<R, Pt, L, IL, X>,
        >,
    X: LendContext,
    N::Cols: Run<R, Pt, X>,
    F: for<'v> FnOnce(N::View<'v, P>) -> O,
{
    let mut tab = <R as KindTab<Pt, frunk_core::hlist::HNil>>::Table::default();
    R::fill(root, frunk_core::hlist::HNil, &mut tab);
    if !R::rows(&tab).iter().any(|(f, _)| f == at) {
        return None;
    }
    let mut row = cx.row();
    let ex = <N::Cols as Run<R, Pt, X>>::take(&mut tab, &mut row);
    let store = <N::Cols as Run<R, Pt, X>>::gather(&tab);
    let hs = <N::Cols as Run<R, Pt, X>>::handles(ex, &store);
    Some(f(N::build(hs, at)))
}

/// [`lend`], handing the view to code generic over its position.
pub(crate) fn lend_visit<R, Pt: 'static, L, IL, N, P, X, V>(
    root: &mut Node<R, Pt>,
    at: &PointFk<R, Pt, L, IL, X>,
    cx: &mut X,
    v: V,
) -> Option<V::Out>
where
    R: KindFor<Pt> + NodeKind + Locate<L, IL>,
    PointCx<R, Pt, L, IL, X>: Ctx,
    N: NoCycle<R, Pt, frunk_core::hlist::HNil>,
    N: Check<PointCx<R, Pt, L, IL, X>, P>,
    N: Takes<PointCx<R, Pt, L, IL, X>, P>,
    <N as Takes<PointCx<R, Pt, L, IL, X>, P>>::Touches: Disjoint + DeepEnoughAll,
    N: Lend<PointCx<R, Pt, L, IL, X>, P>,
    P: Pos,
    R: TabAt<
            Pt,
            frunk_core::hlist::HNil,
            <R as Locate<L, IL>>::Route,
            AtFk = PointFk<R, Pt, L, IL, X>,
        >,
    X: LendContext,
    N::Cols: Run<R, Pt, X>,
    V: Visit<N>,
{
    lend::<R, Pt, L, IL, N, P, X, _, _>(root, at, cx, |view| v.visit::<P>(view))
}

/// Prove the needs `N` at the kind `L` of the tree whose root is `R`, for
/// the parts `Pt`, with no context, without lending them: what the checks of a lending
/// report, for the examples of the documentation.
#[doc(hidden)]
#[allow(private_bounds)]
pub fn assert_lends<R, Pt, L, N, IL, P>()
where
    R: KindFor<Pt> + NodeKind + Locate<L, IL>,
    PointCx<R, Pt, L, IL, ()>: Ctx,
    N: NoCycle<R, Pt, frunk_core::hlist::HNil>,
    N: Check<PointCx<R, Pt, L, IL, ()>, P>,
    N: Takes<PointCx<R, Pt, L, IL, ()>, P>,
    <N as Takes<PointCx<R, Pt, L, IL, ()>, P>>::Touches: Disjoint + DeepEnoughAll,
    N: Lend<PointCx<R, Pt, L, IL, ()>, P>,
    P: Pos,
{
}

/// Prove the needs `N` at the kind `L` of the tree whose root is `R`, for
/// the parts `Pt`, lent with the context `X`, without lending them: what
/// the checks of a lending report, for the examples of the documentation.
#[doc(hidden)]
#[allow(private_bounds)]
pub fn assert_lends_in<R, Pt, X, L, N, IL, P>()
where
    R: KindFor<Pt> + NodeKind + Locate<L, IL>,
    PointCx<R, Pt, L, IL, X>: Ctx,
    N: NoCycle<R, Pt, frunk_core::hlist::HNil>,
    N: Check<PointCx<R, Pt, L, IL, X>, P>,
    N: Takes<PointCx<R, Pt, L, IL, X>, P>,
    <N as Takes<PointCx<R, Pt, L, IL, X>, P>>::Touches: Disjoint + DeepEnoughAll,
    N: Lend<PointCx<R, Pt, L, IL, X>, P>,
    P: Pos,
    X: LendContext,
{
}

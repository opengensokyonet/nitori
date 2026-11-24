//! Lending at a node not built yet: what the components of a new node take
//! to build it, from the nodes above it.
//!
//! The lend point is the new node, so its own key and address are its
//! own; it has no components yet, so a need finds nothing in it, and every
//! component is taken from the nodes above, which are in the tree.
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

use super::{
    Lend, LendContext,
    check::{Check, DeepEnoughAll, Disjoint, Takes},
    cycle::NoCycle,
    line::AtPoint,
    list::{Prefix, Z},
    plan::Run,
    table::{KidRoute, KindTab, TabAt},
    tree::{ChainOf, Ctx, Locate, NoParent, On},
};
use crate::{
    need::Pos,
    structure::{KindFor, Node, NodeKind},
};

/// The new node at the route `Rt` from the root `R`, for the parts `Pt`,
/// lent with the context `Xc`.
pub struct NewCx<R, Pt, Rt, Xc>(PhantomData<fn() -> (R, Pt, Rt, Xc)>);

/// A chain whose last node has no components yet.
pub trait Unbuilt {
    /// The chain.
    type Out;
}
impl<K, Cs, F, Pre, St> Unbuilt for HCons<On<K, Cs, F, Pre, St>, HNil> {
    type Out = HCons<On<K, HNil, F, Pre, St>, HNil>;
}
impl<H, H2, T> Unbuilt for HCons<H, HCons<H2, T>>
where
    HCons<H2, T>: Unbuilt,
{
    type Out = HCons<H, <HCons<H2, T> as Unbuilt>::Out>;
}

impl<R: NodeKind, Pt, Rt, Xc> Ctx for NewCx<R, Pt, Rt, Xc>
where
    (R, Rt): ChainOf<Pt, HNil, HNil, NoParent>,
    <(R, Rt) as ChainOf<Pt, HNil, HNil, NoParent>>::Chain: Unbuilt,
{
    type Root = R;
    type Parts = Pt;
    type Route = Rt;
    type Depth = Z;
    type Chain = <<(R, Rt) as ChainOf<Pt, HNil, HNil, NoParent>>::Chain as Unbuilt>::Out;
    type Fk = <(R, Rt) as ChainOf<Pt, HNil, HNil, NoParent>>::Fk;
    type Kind = <(R, Rt) as ChainOf<Pt, HNil, HNil, NoParent>>::Kind;
    type Storage = <(R, Rt) as ChainOf<Pt, HNil, HNil, NoParent>>::Storage;
    type Context = Xc;
}

/// The route to the parent of the kind at the route `Self`, which is not
/// the root.
pub trait RouteParent {
    /// The parent's route.
    type Out;
}
impl<J> RouteParent for HCons<J, HNil> {
    type Out = HNil;
}
impl<J, J2, T> RouteParent for HCons<J, HCons<J2, T>>
where
    HCons<J2, T>: RouteParent,
{
    type Out = HCons<J, <HCons<J2, T> as RouteParent>::Out>;
}

/// The new node of the kind `L` in the tree whose root is `R`, for the
/// parts `Pt`, lent with the context `Xc`.
pub type NewPointCx<R, Pt, L, IL, Xc> = NewCx<R, Pt, <R as Locate<L, IL>>::Route, Xc>;
/// Its full key.
pub type NewPointFk<R, Pt, L, IL, Xc> = <NewPointCx<R, Pt, L, IL, Xc> as Ctx>::Fk;
/// The route to its parent.
pub type ParentRoute<R, L, IL> = <<R as Locate<L, IL>>::Route as RouteParent>::Out;

/// Lend the needs `N` at a new node of the kind `L` with the full key
/// `at`, not in the tree yet, to `f`, with the context `cx`; `None` when
/// its parent is not in the tree.
pub(crate) fn lend_new<R, Pt: 'static, L, IL, N, P, X, F, O>(
    root: &mut Node<R, Pt>,
    at: &NewPointFk<R, Pt, L, IL, X>,
    cx: &mut X,
    f: F,
) -> Option<O>
where
    R: KindFor<Pt> + NodeKind + Locate<L, IL>,
    NewPointCx<R, Pt, L, IL, X>: Ctx,
    N: NoCycle<R, Pt, HNil>,
    N: Check<NewPointCx<R, Pt, L, IL, X>, P>,
    N: Takes<NewPointCx<R, Pt, L, IL, X>, P>,
    <N as Takes<NewPointCx<R, Pt, L, IL, X>, P>>::Touches: Disjoint + DeepEnoughAll,
    N: Lend<NewPointCx<R, Pt, L, IL, X>, P>,
    P: Pos,
    <R as Locate<L, IL>>::Route: RouteParent,
    R: TabAt<Pt, HNil, ParentRoute<R, L, IL>>,
    NewPointFk<R, Pt, L, IL, X>: Prefix<<R as TabAt<Pt, HNil, ParentRoute<R, L, IL>>>::AtFk>,
    R::Children: KidRoute<
            Pt,
            HNil,
            ParentRoute<R, L, IL>,
            <R as TabAt<Pt, HNil, ParentRoute<R, L, IL>>>::AtFk,
        >,
    X: LendContext,
    N::Cols: Run<R, Pt, X>,
    F: for<'v> FnOnce(N::View<'v, P>) -> O,
{
    let mut tab = <R as KindTab<Pt, HNil>>::Table::default();
    <N::Cols as Run<R, Pt, X>>::mark(&mut tab);
    R::settle(&mut tab);
    let parent: <R as TabAt<Pt, HNil, ParentRoute<R, L, IL>>>::AtFk = at.prefix();
    let mut walk = 0;
    if !R::fill_route::<ParentRoute<R, L, IL>, _>(root, HNil, &parent, &mut tab, &mut walk, false) {
        return None;
    }
    let mut row = cx.row();
    let ex = <N::Cols as Run<R, Pt, X>>::take::<AtPoint>(&mut tab, &mut row);
    let store = <N::Cols as Run<R, Pt, X>>::gather::<AtPoint>(&tab);
    let hs = <N::Cols as Run<R, Pt, X>>::handles(ex, &store);
    Some(f(N::build(hs, at)))
}

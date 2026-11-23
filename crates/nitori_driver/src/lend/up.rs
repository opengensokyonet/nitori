//! Lists of needs, the needs taken on the way up, and moving the lend
//! point.
use alloc::vec::Vec;

use frunk_core::hlist::{HCons, HNil};

use super::{
    Lend,
    list::{HasPrefix, Prefix},
    plan::{ExCol, Plan, ShCol, Shared},
    tree::{AtPos, Ctx, Cx, DepthGe, FindUp, UpPos},
};
use crate::need::{At, AtTarget, ExactlyOne, Mut, Pos, Ref};

/// The kind of the node up the chain of `C` that has `X`, found at `I`.
pub type UpKind<C, X, I> = <<C as Ctx>::Chain as FindUp<X, I>>::Kind;
/// Its full key.
pub type UpFk<C, X, I> = <<C as Ctx>::Chain as FindUp<X, I>>::Fk;
/// The route from the root to it.
pub type UpPre<C, X, I> = <<C as Ctx>::Chain as FindUp<X, I>>::Pre;
/// The position of `X` among its components.
pub type UpCol<C, X, I> = <<C as Ctx>::Chain as FindUp<X, I>>::Col;

impl<C: Ctx> Lend<C, HNil> for HNil {
    type Cols = ();
    fn build<'a>(_: (), _: &C::Fk) -> HNil
    where
        Self: 'a,
    {
        HNil
    }
}
impl<C: Ctx, N, T, PN: Pos, PT: Pos> Lend<C, HCons<PN, PT>> for HCons<N, T>
where
    N: Lend<C, PN>,
    T: Lend<C, PT>,
{
    type Cols = (N::Cols, T::Cols);
    fn build<'a>(
        (a, b): <Self::Cols as Plan>::Hs<'a>,
        at: &C::Fk,
    ) -> HCons<N::View<'a, PN>, T::View<'a, PT>>
    where
        Self: 'a,
    {
        HCons {
            head: N::build(a, at),
            tail: T::build(b, at),
        }
    }
}

impl<C: Ctx, X: 'static, I: 'static> Lend<C, UpPos<I>> for Mut<X>
where
    C::Chain: FindUp<X, I>,
    I: DepthGe<C::Depth>,
    C::Fk: HasPrefix<UpFk<C, X, I>>,
    UpPre<C, X, I>: 'static,
    UpCol<C, X, I>: 'static,
{
    type Cols = ExCol<UpPre<C, X, I>, UpCol<C, X, I>, UpFk<C, X, I>, X>;
    fn build<'a>(hs: Vec<(UpFk<C, X, I>, &'a mut X)>, at: &C::Fk) -> &'a mut X
    where
        Self: 'a,
    {
        // Invariant: the lend point is in the tree, so is every node above
        // it.
        hs.into_iter()
            .find(|(f, _)| at.has_prefix(f))
            .map(|(_, x)| x)
            .expect("the node above a lend point is in the tree")
    }
}

impl<C: Ctx, X: 'static, I: 'static> Lend<C, UpPos<I>> for Ref<X>
where
    C::Chain: FindUp<X, I>,
    C::Fk: HasPrefix<UpFk<C, X, I>>,
    UpPre<C, X, I>: 'static,
    UpCol<C, X, I>: 'static,
{
    type Cols = ShCol<UpPre<C, X, I>, UpCol<C, X, I>, UpFk<C, X, I>, X>;
    fn build<'a>(hs: Shared<'a, UpFk<C, X, I>, X>, at: &C::Fk) -> &'a X
    where
        Self: 'a,
    {
        // Invariant: as for `Mut`.
        hs.iter()
            .find(|(f, _)| at.has_prefix(f))
            .map(|(_, x)| x)
            .expect("the node above a lend point is in the tree")
    }
}

/// The lend point an `At` moves to from `C`: the node up the chain that
/// has `B`, found at `I`, with the same depth of members.
pub type AtCx<C, B, I> =
    Cx<<C as Ctx>::Root, <C as Ctx>::Parts, UpPre<C, B, I>, <C as Ctx>::Depth, <C as Ctx>::Context>;

impl<C: Ctx, B: AtTarget<Count = ExactlyOne> + 'static, T, I: 'static, PT: Pos>
    Lend<C, AtPos<I, PT>> for At<B, T>
where
    C::Chain: FindUp<B, I>,
    AtCx<C, B, I>: Ctx,
    T: Lend<AtCx<C, B, I>, PT>,
    C::Fk: Prefix<<AtCx<C, B, I> as Ctx>::Fk>,
{
    type Cols = T::Cols;
    fn build<'a>(hs: <T::Cols as Plan>::Hs<'a>, at: &C::Fk) -> T::View<'a, PT>
    where
        Self: 'a,
    {
        T::build(hs, &at.prefix())
    }
}

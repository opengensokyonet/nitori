//! The waker of a component: on the way up, or of each member.
use core::marker::PhantomData;

use frunk_core::hlist::HNil;

use super::{
    Lend,
    check::{
        Check, CheckBody, CountChain, ExactlyOneUp, Spell, Takes, TakesBody, TakesExclusive,
        UpCount,
    },
    ident::{IdentCol, Idents},
    list::{HasPrefix, Len},
    plan::PartPlan,
    tree::{AtMember, Collect, Ctx, FindUp},
    under::{Body, Gather, UnderBody, by_shape},
    up::{UpFk, UpPre},
};
use crate::{
    id::{NumberValue, Numbered, number_of},
    need::{ComponentWaker, Pos, WakerOf},
};

/// The position of a [`WakerOf`]: the node found up the chain at `I`, or
/// the member ([`AtMember`]).
pub struct WakerPos<I>(PhantomData<fn() -> I>);
impl<I: 'static> Pos for WakerPos<I> {}

impl<C: Ctx, X: Numbered<Number: NumberValue> + 'static, I: 'static> Lend<C, WakerPos<I>>
    for WakerOf<X>
where
    C::Chain: FindUp<X, I>,
    C::Fk: HasPrefix<UpFk<C, X, I>>,
    UpPre<C, X, I>: 'static,
{
    type Cols = IdentCol<UpPre<C, X, I>, UpFk<C, X, I>>;
    fn build<'a>(hs: Idents<'a, UpFk<C, X, I>>, at: &C::Fk) -> ComponentWaker
    where
        Self: 'a,
    {
        // Invariant: the lend point is in the tree, so is every node above
        // it.
        let node = hs
            .iter()
            .find(|(f, _, _)| at.has_prefix(f))
            .map(|(_, ident, _)| *ident)
            .expect("the node above a lend point is in the tree");
        ComponentWaker::new(node, number_of::<X>())
    }
}
impl<C: Ctx, X, I> Check<C, WakerPos<I>> for WakerOf<X>
where
    C::Chain: CountChain<X>,
    <C::Chain as CountChain<X>>::N: Spell,
    UpCount<C, X>: ExactlyOneUp<X>,
{
}
impl<C, X, I> Takes<C, WakerPos<I>> for WakerOf<X> {
    type Touches = HNil;
}

impl<X: 'static> UnderBody for WakerOf<X> {
    type B = X;
    type Depth<Rt: Len, Sd> = Rt::N;
}
impl<MC: Ctx, X: Numbered<Number: NumberValue> + 'static> Body<MC, WakerPos<AtMember>>
    for WakerOf<X>
where
    MC::Route: 'static,
{
    type Cols = IdentCol<MC::Route, MC::Fk>;
    fn build<'a>(hs: Idents<'a, MC::Fk>, member: &MC::Fk) -> ComponentWaker
    where
        Self: 'a,
    {
        // Invariant: the members are the nodes of the kind at the route,
        // so every member has a row.
        let node = hs
            .iter()
            .find(|(f, _, _)| *f == member)
            .map(|(_, ident, _)| *ident)
            .expect("a member of an `Under` is in the tree");
        ComponentWaker::new(node, number_of::<X>())
    }
}
impl<MC: Ctx, AF, LF, IA: 'static, IK: 'static, Sh, PB: Pos, X: 'static>
    Gather<MC, AF, LF, super::tree::UnderPos<IA, IK, Sh, PB>> for WakerOf<X>
where
    Self: Body<MC, PB>,
    <Self as Body<MC, PB>>::Cols: PartPlan<MC::Route>,
    Sh: Collect<AF, MC::Fk, LF>,
{
    type Cols = <Self as Body<MC, PB>>::Cols;
    fn gather<'a>(
        rows: &'a [(&'a MC::Fk, super::rows::Span)],
        hs: <Self::Cols as super::plan::Plan>::Hs<'a>,
        anchor: &AF,
        at: &LF,
        line: bool,
    ) -> Self::Collected<'a, super::tree::UnderPos<IA, IK, Sh, PB>>
    where
        Self: 'a,
    {
        by_shape::<Self, MC, AF, LF, Sh, PB>(rows, hs, anchor, at, line)
    }
}
impl<MC, X, I> CheckBody<MC, WakerPos<I>> for WakerOf<X> {}
impl<X> TakesExclusive for WakerOf<X> {
    type Out = crate::id::False;
}
impl<MC, X, I> TakesBody<MC, WakerPos<I>> for WakerOf<X> {
    type Touches = HNil;
}

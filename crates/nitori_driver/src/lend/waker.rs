//! The waker of a component: on the way up, or of each member.
use core::marker::PhantomData;

use frunk_core::hlist::HNil;

use super::{
    Lend,
    check::{Check, CheckBody, CountChain, ExactlyOneUp, Spell, Takes, TakesBody, UpCount},
    list::{Len, Prefix},
    plan::PartPlan,
    tree::{AtMember, Collect, Ctx, FindUp},
    under::{Body, Gather, UnderBody},
    up::{UpFk, UpPre},
};
use crate::need::{ComponentWaker, Pos, WakerOf};

/// The position of a [`WakerOf`], lent as `W`: the node found up the
/// chain at `I`, or the member ([`AtMember`]).
pub struct WakerPos<I, W>(PhantomData<fn() -> (I, W)>);
impl<I: 'static, W: Clone + PartialEq + 'static> Pos for WakerPos<I, W> {
    type Waker = W;
}

/// The waker of `X` of the node found up the chain of `C` at `I`.
pub type UpWaker<C, X, I> = ComponentWaker<X, UpPre<C, X, I>, UpFk<C, X, I>>;

impl<C: Ctx, X: 'static, I: 'static> Lend<C, WakerPos<I, UpWaker<C, X, I>>> for WakerOf<X>
where
    C::Chain: FindUp<X, I>,
    C::Fk: Prefix<UpFk<C, X, I>>,
    UpPre<C, X, I>: 'static,
{
    type Cols = ();
    fn build<'a>(_: (), at: &C::Fk) -> UpWaker<C, X, I>
    where
        Self: 'a,
    {
        ComponentWaker::new(at.prefix())
    }
}
impl<C: Ctx, X, I, W> Check<C, WakerPos<I, W>> for WakerOf<X>
where
    C::Chain: CountChain<X>,
    <C::Chain as CountChain<X>>::N: Spell,
    UpCount<C, X>: ExactlyOneUp<X>,
{
}
impl<C, X, I, W> Takes<C, WakerPos<I, W>> for WakerOf<X> {
    type Touches = HNil;
}

/// The waker of `X` of each member, at the lend point `MC`.
pub type MemberWaker<MC, X> = ComponentWaker<X, <MC as Ctx>::Route, <MC as Ctx>::Fk>;

impl<X: 'static> UnderBody for WakerOf<X> {
    type B = X;
    type Depth<Rt: Len> = Rt::N;
}
impl<MC: Ctx, X: 'static> Body<MC, WakerPos<AtMember, MemberWaker<MC, X>>> for WakerOf<X>
where
    MC::Route: 'static,
{
    type Cols = ();
    fn build<'a>(_: (), member: &MC::Fk) -> MemberWaker<MC, X>
    where
        Self: 'a,
    {
        ComponentWaker::new(member.clone())
    }
}
impl<MC: Ctx, AF, LF, IA: 'static, IK: 'static, Sh, PB: Pos, X: 'static>
    Gather<MC, AF, LF, super::tree::UnderPos<IA, IK, Sh, PB>> for WakerOf<X>
where
    Self: Body<MC, PB, Cols = ()>,
    (): PartPlan<MC::Fk>,
    Sh: Collect<AF, MC::Fk, LF>,
{
    type Cols = ();
    fn gather<'a>(
        rows: &'a [&'a MC::Fk],
        _: (),
        anchor: &AF,
        at: &LF,
    ) -> Self::Collected<'a, super::tree::UnderPos<IA, IK, Sh, PB>>
    where
        Self: 'a,
    {
        Sh::collect(rows, anchor, at, |m| <Self as Body<MC, PB>>::build((), m))
    }
}
impl<MC, X, W> CheckBody<MC, WakerPos<AtMember, W>> for WakerOf<X> {}
impl<MC, X, W> TakesBody<MC, WakerPos<AtMember, W>> for WakerOf<X> {
    type Touches = HNil;
}

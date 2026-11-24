//! Capabilities lent: a composed capability's needs, put together into
//! its object, and every part's piece of a contributed one.
use core::marker::PhantomData;

use super::{
    Lend,
    check::{Check, Takes, TakesExclusive},
    plan::Plan,
    tree::Ctx,
};
use crate::{
    capability::{Capability, Contributed, PieceList},
    need::{Cap, Pos},
};

/// The position of a [`Cap`]: its needs at `P`.
pub struct CapPos<P>(PhantomData<fn() -> P>);
impl<P: Pos> Pos for CapPos<P> {
    type Inner = P;
}

impl<C: Ctx, X: Capability, PN: Pos> Lend<C, CapPos<PN>> for Cap<X>
where
    X::Needs: Lend<C, PN>,
{
    type Cols = <X::Needs as Lend<C, PN>>::Cols;
    fn build<'a>(hs: <Self::Cols as Plan>::Hs<'a>, at: &C::Fk) -> X::Object<'a, PN>
    where
        Self: 'a,
    {
        X::assemble::<PN>(<X::Needs as Lend<C, PN>>::build(hs, at))
    }
}
impl<C, X: Capability, PN> Check<C, CapPos<PN>> for Cap<X> where X::Needs: Check<C, PN> {}
impl<C, X: Capability, PN> Takes<C, CapPos<PN>> for Cap<X>
where
    X::Needs: Takes<C, PN>,
{
    type Touches = <X::Needs as Takes<C, PN>>::Touches;
}
impl<X: Capability> TakesExclusive for Cap<X>
where
    X::Needs: TakesExclusive,
{
    type Out = <X::Needs as TakesExclusive>::Out;
}

/// The position of a [`Contributed`]: the pieces' needs at `Ps`.
pub struct ContribPos<Ps>(PhantomData<fn() -> Ps>);
impl<Ps: Pos> Pos for ContribPos<Ps> {
    type Inner = Ps;
}

impl<C: Ctx, F: 'static, L: PieceList<F>, Ps: Pos> Lend<C, ContribPos<Ps>> for Contributed<F, L>
where
    L::Needs: Lend<C, Ps>,
{
    type Cols = <L::Needs as Lend<C, Ps>>::Cols;
    fn build<'a>(hs: <Self::Cols as Plan>::Hs<'a>, at: &C::Fk) -> L::Pieces<'a>
    where
        Self: 'a,
    {
        L::assemble::<Ps>(<L::Needs as Lend<C, Ps>>::build(hs, at))
    }
}
impl<C, F, L: PieceList<F>, Ps> Check<C, ContribPos<Ps>> for Contributed<F, L> where
    L::Needs: Check<C, Ps>
{
}
impl<C, F, L: PieceList<F>, Ps> Takes<C, ContribPos<Ps>> for Contributed<F, L>
where
    L::Needs: Takes<C, Ps>,
{
    type Touches = <L::Needs as Takes<C, Ps>>::Touches;
}
impl<F, L: PieceList<F>> TakesExclusive for Contributed<F, L>
where
    L::Needs: TakesExclusive,
{
    type Out = <L::Needs as TakesExclusive>::Out;
}

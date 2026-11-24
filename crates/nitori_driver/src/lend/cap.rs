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
    id::True,
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

/// The position of a [`Contributed`]: the parts `Pt`, whose pieces for
/// the family `F` it holds, and their needs at `Ps`.
pub struct ContribPos<Pt, F, Ps>(PhantomData<fn() -> (Pt, F, Ps)>);
impl<Pt: PieceList<F>, F: 'static, Ps: Pos> Pos for ContribPos<Pt, F, Ps> {
    type Inner = Ps;
    type Held<'t> = Pt::Pieces<'t>;
}

impl<C, Pt, F: 'static, Ps: Pos> Lend<C, ContribPos<Pt, F, Ps>> for Contributed<F>
where
    C: Ctx<Parts = Pt>,
    Pt: PieceList<F>,
    Pt::Needs: Lend<C, Ps>,
{
    type Cols = <Pt::Needs as Lend<C, Ps>>::Cols;
    fn build<'a>(hs: <Self::Cols as Plan>::Hs<'a>, at: &C::Fk) -> Pt::Pieces<'a>
    where
        Self: 'a,
    {
        Pt::assemble::<Ps>(<Pt::Needs as Lend<C, Ps>>::build(hs, at))
    }
}
impl<C: Ctx<Parts = Pt>, Pt: PieceList<F>, F, Ps> Check<C, ContribPos<Pt, F, Ps>> for Contributed<F> where
    Pt::Needs: Check<C, Ps>
{
}
impl<C: Ctx<Parts = Pt>, Pt: PieceList<F>, F, Ps> Takes<C, ContribPos<Pt, F, Ps>> for Contributed<F>
where
    Pt::Needs: Takes<C, Ps>,
{
    type Touches = <Pt::Needs as Takes<C, Ps>>::Touches;
}
/// What the pieces take follows from the parts, not from the need: taken
/// as exclusive.
impl<F> TakesExclusive for Contributed<F> {
    type Out = True;
}

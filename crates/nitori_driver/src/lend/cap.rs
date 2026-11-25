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
    capability::{Capability, Contributed, PieceList, contribution::PieceLists},
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

impl<C: Ctx, F: 'static, Q: PieceList<F>, Ps: Pos> Lend<C, ContribPos<Ps>> for Contributed<F, Q>
where
    Q::Needs: Lend<C, Ps>,
{
    type Cols = <Q::Needs as Lend<C, Ps>>::Cols;
    fn build<'a>(hs: <Self::Cols as Plan>::Hs<'a>, at: &C::Fk) -> Q::Pieces<'a>
    where
        Self: 'a,
    {
        Q::assemble::<Ps>(<Q::Needs as Lend<C, Ps>>::build(hs, at))
    }
}
impl<C, F, Q, Ps> Check<C, ContribPos<Ps>> for Contributed<F, Q>
where
    C: Ctx,
    Q: PieceList<F> + PieceLists,
    Q::Needs: Check<C, Ps>,
    C::Parts: LentUnder<Q, <Q as PieceLists>::Parts>,
{
}
impl<C, F, Q: PieceList<F>, Ps> Takes<C, ContribPos<Ps>> for Contributed<F, Q>
where
    Q::Needs: Takes<C, Ps>,
{
    type Touches = <Q::Needs as Takes<C, Ps>>::Touches;
}
impl<F, Q: PieceList<F>> TakesExclusive for Contributed<F, Q>
where
    Q::Needs: TakesExclusive,
{
    type Out = <Q::Needs as TakesExclusive>::Out;
}

/// The parts `Self` of the structure a [`Contributed`] is lent in are
/// those of its composition `Q`, `P`.
#[diagnostic::on_unimplemented(
    message = "`Contributed<_, {Q}>` is lent in a structure whose parts are not those of `{Q}`",
    note = "the pieces are those of the parts of the composition the need names: in a call, take `Contributed<F, Q>` of the composition `Q` the call is lent under"
)]
pub trait LentUnder<Q, P> {}
impl<Q, P> LentUnder<Q, P> for P {}

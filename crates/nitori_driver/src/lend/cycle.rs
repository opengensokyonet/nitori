//! That reaction points do not chain into a cycle, with its reason.
//!
//! A reaction that fires another point takes that point's reactions, and
//! a chain that comes back to a point it started from would expand without
//! end, which the proof of the needs reports only as an overflow. The
//! check walks the needs before, carrying the points it went through and
//! comparing them by number. It looks at every kind of the structure, not
//! only the kinds around the lend point, so a chain through reactions that
//! the lend point does not take is refused as well.
use frunk_core::hlist::{HCons, HNil};

use super::{
    react::{AllKinds, ListReactions, ReactionNeeds},
    this::This,
};
use crate::{
    capability::{Capability, Contributed, PieceList},
    id::{False, NumberEq},
    need::{
        At, Clock, Mut, OneAtATime, OwnAddress, OwnKey, Plus, Ref, Requests, Under, WakerOf, Wakes,
    },
    react::{Point, Reactions},
};

/// The point `Self` differs from the point `P` a chain went through.
#[diagnostic::on_unimplemented(
    message = "reaction points chain into a cycle: a reaction to `{P}` needs `Cap<{P}>`, directly or through other points",
    label = "`{P}` comes again on the chain",
    note = "firing a point takes its reactions, and a reaction that fires a point the chain went through expands without end"
)]
pub trait Differs<P> {}
impl<P> Differs<P> for False {}

/// The point `Self` is not among the points `Seen` of a chain.
#[diagnostic::on_unimplemented(
    message = "reaction points chain into a cycle: a reaction to `{Self}` needs `Cap<{Self}>`, directly or through other points",
    label = "`{Self}` comes again on the chain",
    note = "the chain so far is `{Seen}`, the latest first; firing a point takes its reactions, and a reaction that fires a point the chain went through expands without end"
)]
pub trait NotIn<Seen> {}
impl<P> NotIn<HNil> for P {}
impl<P: Point, Q: Point, T> NotIn<HCons<Q, T>> for P
where
    P::Number: NumberEq<Q::Number>,
    <P::Number as NumberEq<Q::Number>>::Out: Differs<P>,
    P: NotIn<T>,
{
}

/// The need `Self` fires no point that the chain `Seen` went through, in a
/// tree whose root is `Rt` for the parts `Pt`.
#[diagnostic::on_unimplemented(
    message = "reaction points chain into a cycle: `{Self}` fires a point the chain `{Seen}` went through",
    label = "the chain comes back here",
    note = "firing a point takes its reactions, and a reaction that fires a point the chain went through expands without end"
)]
pub trait NoCycle<Rt, Pt, Seen> {}
impl<Rt, Pt, S> NoCycle<Rt, Pt, S> for HNil {}
impl<Rt, Pt, S, N: NoCycle<Rt, Pt, S>, T: NoCycle<Rt, Pt, S>> NoCycle<Rt, Pt, S> for HCons<N, T> {}
macro_rules! fires_nothing {
    ($($t:ty $([$($g:ident),*])?;)*) => {$(
        impl<Rt, Pt, S $($(, $g)*)?> NoCycle<Rt, Pt, S> for $t {}
    )*};
}
fires_nothing! {
    Mut<X> [X];
    Ref<X> [X];
    WakerOf<X> [X];
    This<X> [X];
    OwnKey<K> [K];
    OwnAddress<F> [F];
    Clock<C> [C];
    Requests<R> [R];
    Wakes<P> [P];
}
impl<Rt, Pt, S, B, T: NoCycle<Rt, Pt, S>> NoCycle<Rt, Pt, S> for At<B, T> {}
impl<Rt, Pt, S, A, T: NoCycle<Rt, Pt, S>> NoCycle<Rt, Pt, S> for Under<A, T> {}
impl<Rt, Pt, S, T: NoCycle<Rt, Pt, S>> NoCycle<Rt, Pt, S> for OneAtATime<T> {}
impl<Rt, Pt, S, Bs: NoCycle<Rt, Pt, S>> NoCycle<Rt, Pt, S> for Plus<Bs> {}
impl<Rt, Pt, S, X> NoCycle<Rt, Pt, S> for crate::need::Cap<X> where X: CapNoCycle<Rt, Pt, S> {}
/// The capability `Self` fires no point that the chain `Seen` went
/// through. Its own bound, so that a type that is not a capability is
/// reported as such rather than as a cycle.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a capability",
    note = "a capability taken with `Cap` is a composed one, implementing `Capability`, or a part's reaction point, implementing `ReactionPoint`; the driver's own points `NodeStarted` and `NodeEnding` are fired by the driver only"
)]
pub trait CapNoCycle<Rt, Pt, Seen> {}
impl<Rt, Pt, S, X: Capability> CapNoCycle<Rt, Pt, S> for X where X::Needs: NoCycle<Rt, Pt, S> {}
impl<Rt, Pt, S, F, L> NoCycle<Rt, Pt, S> for Contributed<F, L> where L: PiecesNoCycle<F, Rt, Pt, S> {}
/// The pieces of the parts `Self` for the family `F` fire no point that
/// the chain `Seen` went through. Its own bound, so that a part that does
/// not contribute is reported as such rather than as a cycle.
#[diagnostic::on_unimplemented(
    message = "a part of `{Self}` does not say what it contributes to the family `{F}`",
    note = "every part writes `Contribute<{F}>` when a need of the composition takes `Contributed<{F}, _>`; `Needs = HNil` and `Piece<'t> = ()` for nothing"
)]
pub trait PiecesNoCycle<F, Rt, Pt, Seen> {}
impl<Rt, Pt, S, F, L> PiecesNoCycle<F, Rt, Pt, S> for L
where
    L: PieceList<F>,
    L::Needs: NoCycle<Rt, Pt, S>,
{
}
impl<Rt, Pt, S, P, W> NoCycle<Rt, Pt, S> for Reactions<P, W>
where
    P: Point + NotIn<S>,
    Rt: AllKinds,
    Pt: ListReactions<P, Rt::Kinds, Pt>,
    <Pt as ListReactions<P, Rt::Kinds, Pt>>::Rs: ReactionNeeds<P>,
    <<Pt as ListReactions<P, Rt::Kinds, Pt>>::Rs as ReactionNeeds<P>>::Needs:
        NoCycle<Rt, Pt, HCons<P, S>>,
{
}

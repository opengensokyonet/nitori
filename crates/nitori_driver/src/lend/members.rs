//! A list of needs taken at each member of an `Under`: the members are
//! the nodes of the one kind that has every component the list takes, and
//! each need is taken at the member, as it is alone.
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

use super::{
    Lend,
    check::{Check, CheckBody, Takes, TakesBody},
    context::CxPos,
    list::{Append, Len},
    plan::{PartPlan, Plan},
    tree::{AddressPos, Collect, Ctx, Elem, FoundIn, KeyPos, UnderPos},
    under::{Body, Gather, UnderBody, by_shape},
};
use crate::need::{At, Clock, Mut, Need, OwnAddress, OwnKey, Pos, Ref, Requests, WakerOf, Wakes};

/// The components `L` (an `HList`) all together: the members of an
/// `Under` whose need is a list are the nodes of the one kind that has
/// every one of them.
pub struct AllOf<L>(PhantomData<fn() -> L>);

impl<L: AllAt<Cs, Js>, Cs, Js> FoundIn<Cs, AllPos<Js>> for AllOf<L> {}
/// The positions `Js` of the components of an `AllOf` among a kind's.
pub struct AllPos<Js>(PhantomData<fn() -> Js>);
/// Every component of the list `Self` is among the components `Cs`, at
/// the positions `Js` (inferred).
pub trait AllAt<Cs, Js> {}
impl<Cs> AllAt<Cs, HNil> for HNil {}
impl<X, T: AllAt<Cs, JT>, Cs: Elem<X, J>, J, JT> AllAt<Cs, HCons<J, JT>> for HCons<X, T> {}

/// A need of a list taken at each member of an `Under`, and the
/// components it asks the members to have.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not taken at each member in a list inside `Under`",
    label = "not a need of a list taken at each member",
    note = "in `Under<A, HList![..]>`, each need is `Mut`, `Ref`, `WakerOf`, `At<X, ..>`, `OwnKey`, `OwnAddress`, `Clock`, `Requests` or `Wakes`; take any other need at each member with `Under<A, At<X, HList![..]>>`, and a list one member at a time with `Under<A, OneAtATime<HList![..]>>`"
)]
pub trait MemberItem {
    /// The components the member has: none, or one.
    type Comps;
}
impl<X> MemberItem for Mut<X> {
    type Comps = HCons<X, HNil>;
}
impl<X> MemberItem for Ref<X> {
    type Comps = HCons<X, HNil>;
}
impl<X> MemberItem for WakerOf<X> {
    type Comps = HCons<X, HNil>;
}
impl<B, T> MemberItem for At<B, T> {
    type Comps = HCons<B, HNil>;
}
impl<K> MemberItem for OwnKey<K> {
    type Comps = HNil;
}
impl<F> MemberItem for OwnAddress<F> {
    type Comps = HNil;
}
impl<C> MemberItem for Clock<C> {
    type Comps = HNil;
}
impl<R> MemberItem for Requests<R> {
    type Comps = HNil;
}
impl<P> MemberItem for Wakes<P> {
    type Comps = HNil;
}

/// The components a list of needs taken at each member asks for.
pub trait ItemComps {
    /// The components, with repeats.
    type Out;
}
impl ItemComps for HNil {
    type Out = HNil;
}
impl<H: MemberItem, T: ItemComps> ItemComps for HCons<H, T>
where
    H::Comps: Append<T::Out>,
{
    type Out = <H::Comps as Append<T::Out>>::Out;
}

impl<H: Need, T: Need> UnderBody for HCons<H, T>
where
    Self: ItemComps,
    <Self as ItemComps>::Out: 'static,
{
    type B = AllOf<<Self as ItemComps>::Out>;
    type Depth<Rt: Len, Sd> = Rt::N;
}

impl<MC: Ctx> Body<MC, HNil> for HNil {
    type Cols = ();
    fn build<'a>(_: (), _: &MC::Fk) -> HNil
    where
        Self: 'a,
    {
        HNil
    }
}
impl<MC: Ctx, H, T, PH: Pos, PT: Pos> Body<MC, HCons<PH, PT>> for HCons<H, T>
where
    H: Body<MC, PH>,
    T: Body<MC, PT>,
{
    type Cols = (H::Cols, T::Cols);
    fn build<'a>(
        (h, t): <Self::Cols as Plan>::Hs<'a>,
        member: &MC::Fk,
    ) -> HCons<H::View<'a, PH>, T::View<'a, PT>>
    where
        Self: 'a,
    {
        HCons {
            head: H::build(h, member),
            tail: T::build(t, member),
        }
    }
}

impl<MC: Ctx, AF, IA: 'static, IK: 'static, Sh, PB: Pos, H: Need, T: Need>
    Gather<MC, AF, UnderPos<IA, IK, Sh, PB>> for HCons<H, T>
where
    Self: ItemComps + Body<MC, PB>,
    <Self as ItemComps>::Out: 'static,
    <Self as Body<MC, PB>>::Cols: PartPlan<MC::Route>,
    Sh: Collect<AF, MC::Fk>,
{
    type Cols = <Self as Body<MC, PB>>::Cols;
    fn gather<'a>(
        rows: &'a [(&'a MC::Fk, super::rows::Span)],
        hs: <Self::Cols as Plan>::Hs<'a>,
        anchor: &AF,
        is_own: impl Fn(&MC::Fk) -> bool,
    ) -> Self::Collected<'a, UnderPos<IA, IK, Sh, PB>>
    where
        Self: 'a,
    {
        by_shape::<Self, MC, AF, Sh, PB>(rows, hs, anchor, is_own)
    }
}

impl<MC> CheckBody<MC, HNil> for HNil {}
impl<MC, H: CheckBody<MC, PH>, T: CheckBody<MC, PT>, PH, PT> CheckBody<MC, HCons<PH, PT>>
    for HCons<H, T>
{
}
impl<MC> TakesBody<MC, HNil> for HNil {
    type Touches = HNil;
}
impl<MC, H: TakesBody<MC, PH>, T: TakesBody<MC, PT>, PH, PT> TakesBody<MC, HCons<PH, PT>>
    for HCons<H, T>
where
    H::Touches: Append<T::Touches>,
{
    type Touches = <H::Touches as Append<T::Touches>>::Out;
}

/// The needs that read the member itself or the lending, taken at each
/// member as they are at a lend point.
macro_rules! as_at_member {
    ($([$($g:tt)*] $need:ty, $pos:ty;)*) => {$(
        impl<MC: Ctx, $($g)*> Body<MC, $pos> for $need
        where
            Self: Lend<MC, $pos>,
        {
            type Cols = <Self as Lend<MC, $pos>>::Cols;
            fn build<'a>(
                hs: <Self::Cols as Plan>::Hs<'a>,
                member: &MC::Fk,
            ) -> Self::View<'a, $pos>
            where
                Self: 'a,
            {
                <Self as Lend<MC, $pos>>::build(hs, member)
            }
        }
        impl<MC, $($g)*> CheckBody<MC, $pos> for $need where Self: Check<MC, $pos> {}
        impl<MC, $($g)*> TakesBody<MC, $pos> for $need
        where
            Self: Takes<MC, $pos>,
        {
            type Touches = <Self as Takes<MC, $pos>>::Touches;
        }
    )*};
}
as_at_member! {
    [K: 'static] OwnKey<K>, KeyPos;
    [A: 'static, F: crate::need::AddressForm] OwnAddress<F>, AddressPos<A>;
    [K: nitori_clock::MonotonicClock + 'static, I: 'static] Clock<K>, CxPos<I>;
    [R: 'static, I: 'static] Requests<R>, CxPos<I>;
    [P: 'static, I: 'static] Wakes<P>, CxPos<I>;
}

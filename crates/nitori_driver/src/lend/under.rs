//! The needs taken at every member below an anchor.
use alloc::vec::Vec;

use super::{
    Lend,
    list::{Append, Len, Prefix},
    plan::{ExCol, NodeSet, PartPlan, Plan, ShCol},
    tree::{
        AtMember, AtPos, Collect, Comps, Ctx, DepthGe, Elem, FindBelow, FindUp, FlatData,
        MemberPos, UnderPos,
    },
    up::{UpFk, UpKind, UpPre},
};
use crate::{
    need::{At, Mut, Need, Pos, Ref, Under},
    structure::Shares,
};

/// The component that tells the members of an `Under<A, Self>` apart.
pub trait UnderBody: Need {
    /// The members are the nodes of the one kind that has it.
    type B: 'static;
}
impl<X: 'static> UnderBody for Mut<X> {
    type B = X;
}
impl<X: 'static> UnderBody for Ref<X> {
    type B = X;
}
impl<B: 'static, T: Need> UnderBody for At<B, T> {
    type B = B;
}

/// The need taken at each member, with the member as the lend point `MC`;
/// `P` is its position (inferred).
pub trait Body<MC: Ctx, P: Pos>: Need {
    /// The plan, shared out among the members.
    type Cols: PartPlan<MC::Fk>;
    /// The view at the member with the full key `member`, from its part of
    /// the handles.
    fn build<'a>(hs: <Self::Cols as Plan>::Hs<'a>, member: &MC::Fk) -> Self::View<'a, P>
    where
        Self: 'a;
}

/// The components of the members' kind of the lend point `MC`.
type MemberComps<MC> = Comps<<MC as Ctx>::Kind, <MC as Ctx>::Parts>;

impl<MC: Ctx, X: 'static, J: 'static> Body<MC, MemberPos<J>> for Mut<X>
where
    MC::Parts: Shares<MC::Kind>,
    <MC::Parts as Shares<MC::Kind>>::Data: FlatData,
    MemberComps<MC>: Elem<X, J>,
    MC::Route: 'static,
    MC::Fk: super::list::HasPrefix<MC::Fk>,
{
    type Cols = ExCol<MC::Route, J, MC::Fk, X>;
    fn build<'a>(hs: Vec<(MC::Fk, &'a mut X)>, member: &MC::Fk) -> &'a mut X
    where
        Self: 'a,
    {
        // Invariant: the member's part of the column has its own entry.
        hs.into_iter()
            .find(|(f, _)| f == member)
            .map(|(_, x)| x)
            .expect("a member has its own component")
    }
}
impl<MC: Ctx, X: 'static, J: 'static> Body<MC, MemberPos<J>> for Ref<X>
where
    MC::Parts: Shares<MC::Kind>,
    <MC::Parts as Shares<MC::Kind>>::Data: FlatData,
    MemberComps<MC>: Elem<X, J>,
    MC::Route: 'static,
{
    type Cols = ShCol<MC::Route, J, MC::Fk, X>;
    fn build<'a>(hs: &'a [(&'a MC::Fk, &'a X)], member: &MC::Fk) -> &'a X
    where
        Self: 'a,
    {
        // Invariant: as for `Mut`.
        hs.iter()
            .find(|(f, _)| *f == member)
            .map(|(_, x)| *x)
            .expect("a member has its own component")
    }
}
impl<MC: Ctx, B: 'static, T, PT: Pos> Body<MC, AtPos<AtMember, PT>> for At<B, T>
where
    T: Lend<MC, PT>,
    T::Cols: PartPlan<MC::Fk>,
{
    type Cols = T::Cols;
    fn build<'a>(hs: <T::Cols as Plan>::Hs<'a>, member: &MC::Fk) -> T::View<'a, PT>
    where
        Self: 'a,
    {
        T::build(hs, member)
    }
}

/// The route from the root to the members of an `Under<A, ..>` lent at
/// `C`, whose anchor is found at `IA` and members' kind at `IK`.
pub type MRoute<C, A, IA, X, IK> = <UpPre<C, A, IA> as Append<
    <UpKind<C, A, IA> as FindBelow<<C as Ctx>::Parts, X, IK>>::Rel,
>>::Out;
/// The lend point of each member.
pub type MCx<C, A, IA, X, IK> =
    super::tree::MemberCx<<C as Ctx>::Root, <C as Ctx>::Parts, MRoute<C, A, IA, X, IK>>;
/// The full key of a member.
pub type MFk<C, A, IA, X, IK> = <MCx<C, A, IA, X, IK> as Ctx>::Fk;

type BodyB<Bd> = <Bd as UnderBody>::B;

impl<C: Ctx, A: 'static, Bd, IA: 'static, IK: 'static, Sh, PB: Pos>
    Lend<C, UnderPos<IA, IK, Sh, PB>> for Under<A, Bd>
where
    Bd: UnderBody,
    C::Chain: FindUp<A, IA>,
    IA: DepthGe<C::Depth>,
    UpKind<C, A, IA>: FindBelow<C::Parts, BodyB<Bd>, IK, Sh = Sh>,
    UpPre<C, A, IA>: Append<<UpKind<C, A, IA> as FindBelow<C::Parts, BodyB<Bd>, IK>>::Rel>,
    MRoute<C, A, IA, BodyB<Bd>, IK>: Len + 'static,
    MCx<C, A, IA, BodyB<Bd>, IK>: Ctx,
    Bd: Body<MCx<C, A, IA, BodyB<Bd>, IK>, PB>,
    Sh: Collect<UpFk<C, A, IA>, MFk<C, A, IA, BodyB<Bd>, IK>>,
    C::Fk: Prefix<UpFk<C, A, IA>>,
{
    type Cols = (
        NodeSet<MRoute<C, A, IA, BodyB<Bd>, IK>, MFk<C, A, IA, BodyB<Bd>, IK>>,
        <Bd as Body<MCx<C, A, IA, BodyB<Bd>, IK>, PB>>::Cols,
    );
    fn build<'a>(
        (rows, mut body): <Self::Cols as Plan>::Hs<'a>,
        at: &C::Fk,
    ) -> Self::View<'a, UnderPos<IA, IK, Sh, PB>>
    where
        Self: 'a,
    {
        let anchor: UpFk<C, A, IA> = at.prefix();
        Sh::collect(rows, &anchor, |m| {
            let part = <<Bd as Body<MCx<C, A, IA, BodyB<Bd>, IK>, PB>>::Cols as PartPlan<
                MFk<C, A, IA, BodyB<Bd>, IK>,
            >>::part(&mut body, m);
            <Bd as Body<MCx<C, A, IA, BodyB<Bd>, IK>, PB>>::build(part, m)
        })
    }
}

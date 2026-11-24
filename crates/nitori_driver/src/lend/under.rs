//! The needs taken at every member below an anchor.
use alloc::vec::Vec;

use super::{
    Lend,
    list::{Answer, Append, Len, Prefix, RouteLine},
    plan::{ExCol, NodeSet, PartPlan, Plan, ShCol, Shared},
    tree::{
        AtMember, AtPos, Collect, Comps, Ctx, Cx, Elem, FindBelow, FindUp, FlatData, MemberPos,
        ToShape, UnderPos,
    },
    up::{UpFk, UpKind, UpPre},
};
use crate::{
    need::{At, AtTarget, ExactlyOne, MemberNeed, Mut, Need, Pos, Ref, Shape, Under},
    structure::Shares,
};

/// What tells the members of an `Under<A, Self>` apart, and how far up
/// they may take exclusive.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not taken at each member of an `Under`",
    label = "not a need taken at each member",
    note = "`Under<A, T>` takes `T` at each member: `Mut`, `Ref`, `WakerOf`, `At<X, ..>`, `OneAtATime<..>`, or a list of `Mut`, `Ref`, `WakerOf`, `At<X, ..>`, `OwnKey`, `OwnAddress`, `Clock` and `Requests`; take any other need at each member with `Under<A, At<X, HList![..]>>`"
)]
pub trait UnderBody: MemberNeed {
    /// The members are the nodes of the one kind that has it: a component,
    /// or all the components of a list ([`AllOf`](super::members::AllOf)).
    type B: 'static;
    /// The depth of members the members' needs are lent with, the members
    /// being at the route `Rt` and the `Under` lent where the depth of
    /// members is `Sd`: their own, so that nothing they share is taken
    /// exclusive, or, lent one at a time, `Sd`, so that they take exclusive
    /// nothing that the members of an `Under` around them share.
    type Depth<Rt: Len, Sd>;
}
impl<X: 'static> UnderBody for Mut<X> {
    type B = X;
    type Depth<Rt: Len, Sd> = Rt::N;
}
impl<X: 'static> UnderBody for Ref<X> {
    type B = X;
    type Depth<Rt: Len, Sd> = Rt::N;
}
impl<B: AtTarget + 'static, T: Need> UnderBody for At<B, T> {
    type B = B;
    type Depth<Rt: Len, Sd> = Rt::N;
}

/// The need taken at each member, with the member as the lend point `MC`;
/// `P` is its position (inferred).
pub trait Body<MC: Ctx, P: Pos>: Need {
    /// The plan, shared out among the members, or lent again to one
    /// member at a time.
    type Cols: Plan;
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
    fn build<'a>(hs: Shared<'a, MC::Fk, X>, member: &MC::Fk) -> &'a X
    where
        Self: 'a,
    {
        // Invariant: as for `Mut`.
        hs.iter()
            .find(|(f, _)| *f == member)
            .map(|(_, x)| x)
            .expect("a member has its own component")
    }
}
impl<MC: Ctx, B: AtTarget<Count = ExactlyOne> + 'static, T, PT: Pos> Body<MC, AtPos<AtMember, PT>>
    for At<B, T>
where
    T: Lend<MC, PT>,
{
    type Cols = T::Cols;
    fn build<'a>(hs: <T::Cols as Plan>::Hs<'a>, member: &MC::Fk) -> T::View<'a, PT>
    where
        Self: 'a,
    {
        T::build(hs, member)
    }
}

/// The result of an `Under` at its position `P`, from the full keys of
/// the members' kind `rows` and the handles of what each member takes,
/// with every member as the lend point `MC`; the anchor's full key is
/// `AF`, the lend point's `LF`.
pub trait Gather<MC: Ctx, AF, LF, P: Pos>: UnderBody {
    /// What the members take.
    type Cols: Plan;
    /// The result; `line` is whether the members' kind and the lend
    /// point's are on one line from the root ([`Collect`]).
    fn gather<'a>(
        rows: &'a [&'a MC::Fk],
        hs: <Self::Cols as Plan>::Hs<'a>,
        anchor: &AF,
        at: &LF,
        line: bool,
    ) -> Self::Collected<'a, P>
    where
        Self: 'a;
}

/// The members' views, each from its part of the handles, gathered by the
/// shape `Sh`.
pub(super) fn by_shape<'a, Bd, MC: Ctx, AF, LF, Sh, PB: Pos>(
    rows: &'a [&'a MC::Fk],
    mut hs: <Bd::Cols as Plan>::Hs<'a>,
    anchor: &AF,
    at: &LF,
    line: bool,
) -> <Sh::Shape as Shape>::Out<Bd::View<'a, PB>>
where
    Bd: Body<MC, PB> + 'a,
    Bd::Cols: PartPlan<MC::Fk>,
    Sh: Collect<AF, MC::Fk, LF>,
{
    Sh::collect(rows, anchor, at, line, |m| {
        let part = <Bd::Cols as PartPlan<MC::Fk>>::part(&mut hs, m);
        Bd::build(part, m)
    })
}

impl<MC: Ctx, AF, LF, IA: 'static, IK: 'static, Sh, PB: Pos, X: 'static>
    Gather<MC, AF, LF, UnderPos<IA, IK, Sh, PB>> for Mut<X>
where
    Self: Body<MC, PB>,
    <Self as Body<MC, PB>>::Cols: PartPlan<MC::Fk>,
    Sh: Collect<AF, MC::Fk, LF>,
{
    type Cols = <Self as Body<MC, PB>>::Cols;
    fn gather<'a>(
        rows: &'a [&'a MC::Fk],
        hs: <Self::Cols as Plan>::Hs<'a>,
        anchor: &AF,
        at: &LF,
        line: bool,
    ) -> Self::Collected<'a, UnderPos<IA, IK, Sh, PB>>
    where
        Self: 'a,
    {
        by_shape::<Self, MC, AF, LF, Sh, PB>(rows, hs, anchor, at, line)
    }
}
impl<MC: Ctx, AF, LF, IA: 'static, IK: 'static, Sh, PB: Pos, X: 'static>
    Gather<MC, AF, LF, UnderPos<IA, IK, Sh, PB>> for Ref<X>
where
    Self: Body<MC, PB>,
    <Self as Body<MC, PB>>::Cols: PartPlan<MC::Fk>,
    Sh: Collect<AF, MC::Fk, LF>,
{
    type Cols = <Self as Body<MC, PB>>::Cols;
    fn gather<'a>(
        rows: &'a [&'a MC::Fk],
        hs: <Self::Cols as Plan>::Hs<'a>,
        anchor: &AF,
        at: &LF,
        line: bool,
    ) -> Self::Collected<'a, UnderPos<IA, IK, Sh, PB>>
    where
        Self: 'a,
    {
        by_shape::<Self, MC, AF, LF, Sh, PB>(rows, hs, anchor, at, line)
    }
}
impl<MC: Ctx, AF, LF, IA: 'static, IK: 'static, Sh, PB: Pos, B: AtTarget + 'static, T: Need>
    Gather<MC, AF, LF, UnderPos<IA, IK, Sh, PB>> for At<B, T>
where
    Self: Body<MC, PB>,
    <Self as Body<MC, PB>>::Cols: PartPlan<MC::Fk>,
    Sh: Collect<AF, MC::Fk, LF>,
{
    type Cols = <Self as Body<MC, PB>>::Cols;
    fn gather<'a>(
        rows: &'a [&'a MC::Fk],
        hs: <Self::Cols as Plan>::Hs<'a>,
        anchor: &AF,
        at: &LF,
        line: bool,
    ) -> Self::Collected<'a, UnderPos<IA, IK, Sh, PB>>
    where
        Self: 'a,
    {
        by_shape::<Self, MC, AF, LF, Sh, PB>(rows, hs, anchor, at, line)
    }
}

/// The route from the root to the members of an `Under<A, ..>` lent at
/// `C`, whose anchor is found at `IA` and members' kind, the one that has
/// `X`, at `IK`.
pub type MRoute<C, A, IA, X, IK> = <UpPre<C, A, IA> as Append<
    <UpKind<C, A, IA> as FindBelow<<C as Ctx>::Parts, X, IK>>::Rel,
>>::Out;
/// The lend point of each member of an `Under<A, Bd>`.
pub type MCx<C, A, IA, Bd, IK> = Cx<
    <C as Ctx>::Root,
    <C as Ctx>::Parts,
    MRoute<C, A, IA, BodyB<Bd>, IK>,
    <Bd as UnderBody>::Depth<MRoute<C, A, IA, BodyB<Bd>, IK>, <C as Ctx>::Depth>,
    <C as Ctx>::Context,
>;
/// The full key of a member.
pub type MFk<C, A, IA, Bd, IK> = <MCx<C, A, IA, Bd, IK> as Ctx>::Fk;

type BodyB<Bd> = <Bd as UnderBody>::B;

impl<C: Ctx, A: 'static, Bd, IA: 'static, IK: 'static, Sh, PB: Pos>
    Lend<C, UnderPos<IA, IK, Sh, PB>> for Under<A, Bd>
where
    Bd: UnderBody,
    Sh: ToShape,
    C::Chain: FindUp<A, IA>,
    UpKind<C, A, IA>: FindBelow<C::Parts, BodyB<Bd>, IK, Sh = Sh>,
    UpPre<C, A, IA>: Append<<UpKind<C, A, IA> as FindBelow<C::Parts, BodyB<Bd>, IK>>::Rel>,
    MRoute<C, A, IA, BodyB<Bd>, IK>: Len + RouteLine<C::Route> + 'static,
    <MRoute<C, A, IA, BodyB<Bd>, IK> as RouteLine<C::Route>>::Out: Answer,
    MCx<C, A, IA, Bd, IK>: Ctx,
    Bd: Gather<MCx<C, A, IA, Bd, IK>, UpFk<C, A, IA>, C::Fk, UnderPos<IA, IK, Sh, PB>>,
    C::Fk: Prefix<UpFk<C, A, IA>>,
{
    type Cols =
        (
            NodeSet<MRoute<C, A, IA, BodyB<Bd>, IK>, MFk<C, A, IA, Bd, IK>>,
            <Bd as Gather<
                MCx<C, A, IA, Bd, IK>,
                UpFk<C, A, IA>,
                C::Fk,
                UnderPos<IA, IK, Sh, PB>,
            >>::Cols,
        );
    fn build<'a>(
        (rows, hs): <Self::Cols as Plan>::Hs<'a>,
        at: &C::Fk,
    ) -> Self::View<'a, UnderPos<IA, IK, Sh, PB>>
    where
        Self: 'a,
    {
        let anchor: UpFk<C, A, IA> = at.prefix();
        let line = <<MRoute<C, A, IA, BodyB<Bd>, IK> as RouteLine<C::Route>>::Out as Answer>::YES;
        Bd::gather(rows, hs, &anchor, at, line)
    }
}

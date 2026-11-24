//! Moving the lend point to the node a locator picks.
use alloc::vec::Vec;
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

use super::{
    Lend,
    check::{
        Check, CountChain, CountTree, Deep, DeepIf, ExactlyOneBelow, ExactlyOneUp, Spell, Takes,
        TakesExclusive, Touch, UpCount,
    },
    input::InputNeed,
    list::{Append, Len, Prefix},
    plan::{NodeSet, Plan},
    tree::{Collect, Ctx, Cx, FindBelow, FindUp, ToShape},
    under::MRoute,
    up::{UpFk, UpKind, UpPre},
};
use crate::{
    id::False,
    need::{At, Count, ExactlyOne, Locator, Members, OneOrNone, Pos, Shape, Under},
};

/// The position of an `At` whose lend point a locator picks: the anchor
/// found at `IA`, the candidates' kind at `IK` below it, the shape `Sh` of
/// the candidates, the input at `PI` and the need taken at the node picked
/// at `PT`.
pub struct LocPos<IA, IK, Sh, PI, PT>(PhantomData<fn() -> (IA, IK, Sh, PI, PT)>);
impl<IA: 'static, IK: 'static, Sh: 'static, PI: Pos, PT: Pos> Pos for LocPos<IA, IK, Sh, PI, PT> {
    type Inner = PT;
}

/// The lend point of the candidates of a locator lent at `C`, whose
/// anchor, the node that has `A`, is found at `IA`, and whose kind, the
/// one that has `X`, at `IK`.
pub type LocCx<C, A, IA, X, IK> = Cx<
    <C as Ctx>::Root,
    <C as Ctx>::Parts,
    MRoute<C, A, IA, X, IK>,
    <C as Ctx>::Depth,
    <C as Ctx>::Context,
>;
/// The full key of a candidate.
pub type LocFk<C, A, IA, X, IK> = <LocCx<C, A, IA, X, IK> as Ctx>::Fk;

/// The view of the node picked, under the contract: the view, or the
/// broken contract reported.
pub trait Finish: Count {
    /// `v`, the view of the node picked if one was, under the contract of
    /// the locator named `locator`.
    fn finish<V>(v: Option<V>, locator: &'static str) -> Self::Out<V>;
}
impl Finish for ExactlyOne {
    fn finish<V>(v: Option<V>, locator: &'static str) -> V {
        match v {
            Some(v) => v,
            // The contract is the locator's to keep: a broken one is
            // reported where it is found.
            None => panic!(
                "`At<{locator}, ..>`: the locator is `ExactlyOne` but picked none of its candidates"
            ),
        }
    }
}
impl Finish for OneOrNone {
    fn finish<V>(v: Option<V>, _: &'static str) -> Option<V> {
        v
    }
}

impl<C, L, A, X, T, IA, IK, Sh, PI, PT> Lend<C, LocPos<IA, IK, Sh, PI, PT>> for At<L, T>
where
    C: Ctx,
    L: Locator<Among = Under<A, X>>,
    L::Count: Finish,
    L::Input: Lend<C, PI>,
    A: 'static,
    X: 'static,
    IA: 'static,
    IK: 'static,
    PI: Pos,
    PT: Pos,
    C::Chain: FindUp<A, IA>,
    UpKind<C, A, IA>: FindBelow<C::Parts, X, IK, Sh = Sh>,
    Sh: ToShape + Collect<UpFk<C, A, IA>, LocFk<C, A, IA, X, IK>, C::Fk>,
    <Sh as ToShape>::Shape: Shape<Key = L::Key>,
    UpPre<C, A, IA>: Append<<UpKind<C, A, IA> as FindBelow<C::Parts, X, IK>>::Rel>,
    MRoute<C, A, IA, X, IK>: Len + 'static,
    LocCx<C, A, IA, X, IK>: Ctx,
    T: Lend<LocCx<C, A, IA, X, IK>, PT>,
    C::Fk: Prefix<UpFk<C, A, IA>>,
{
    type Cols = (
        <L::Input as Lend<C, PI>>::Cols,
        (
            NodeSet<MRoute<C, A, IA, X, IK>, LocFk<C, A, IA, X, IK>>,
            T::Cols,
        ),
    );
    fn build<'a>(
        (ins, (rows, ths)): <Self::Cols as Plan>::Hs<'a>,
        at: &C::Fk,
    ) -> Self::View<'a, LocPos<IA, IK, Sh, PI, PT>>
    where
        Self: 'a,
    {
        let input = <L::Input as Lend<C, PI>>::build(ins, at);
        let anchor: UpFk<C, A, IA> = at.prefix();
        let all = Sh::collect(rows, &anchor, at, false, |m| m.clone());
        let (keys, members): (Vec<L::Key>, Vec<LocFk<C, A, IA, X, IK>>) =
            all.iter().map(|(k, m)| (k.clone(), m.clone())).unzip();
        let picked = L::pick::<PI>(input, &keys)
            .and_then(|k| keys.iter().position(|c| *c == k))
            .map(|i| &members[i]);
        let v = picked.map(|m| T::build(ths, m));
        <L::Count as Finish>::finish(v, core::any::type_name::<L>())
    }
}

// ------------------------------------------------------------------ checks

impl<C, L, A, X, T, IA, IK, Sh, PI, PT> Check<C, LocPos<IA, IK, Sh, PI, PT>> for At<L, T>
where
    C: Ctx,
    L: Locator<Among = Under<A, X>>,
    C::Chain: CountChain<A>,
    <C::Chain as CountChain<A>>::N: Spell,
    UpCount<C, A>: ExactlyOneUp<A>,
    Self: CheckAmong<C, IA, IK, Sh, PI, PT>,
{
}
/// The checks of the candidates of a locator, once the anchor is found.
pub trait CheckAmong<C, IA, IK, Sh, PI, PT> {}
impl<C, L, A, X, T, IA, IK, Sh, PI, PT> CheckAmong<C, IA, IK, Sh, PI, PT> for At<L, T>
where
    C: Ctx,
    L: Locator<Among = Under<A, X>>,
    C::Chain: FindUp<A, IA>,
    UpKind<C, A, IA>: CountTree<C::Parts, X>,
    <UpKind<C, A, IA> as CountTree<C::Parts, X>>::N: Spell,
    <<UpKind<C, A, IA> as CountTree<C::Parts, X>>::N as Spell>::Name: ExactlyOneBelow<X>,
    Self: CheckPicked<C, IA, IK, Sh, PI, PT>,
{
}
/// The checks of the input of a locator and of what is taken at the node
/// it picks, once the candidates' kind is found.
pub trait CheckPicked<C, IA, IK, Sh, PI, PT> {}
impl<C, L, A, X, T, IA, IK, Sh, PI, PT> CheckPicked<C, IA, IK, Sh, PI, PT> for At<L, T>
where
    C: Ctx,
    L: Locator<Among = Under<A, X>>,
    C::Chain: FindUp<A, IA>,
    UpKind<C, A, IA>: FindBelow<C::Parts, X, IK, Sh = Sh>,
    Sh: ToShape,
    L::Key: PicksBy<L, <<Sh as ToShape>::Shape as Shape>::Key>,
    L::Input: InputNeed,
    L::Input: Check<C, PI>,
    L::Input: Takes<C, PI>,
    <L::Input as Takes<C, PI>>::Touches: OnlyReads,
    UpPre<C, A, IA>: Append<<UpKind<C, A, IA> as FindBelow<C::Parts, X, IK>>::Rel>,
    MRoute<C, A, IA, X, IK>: Len,
    LocCx<C, A, IA, X, IK>: Ctx,
    T: Check<LocCx<C, A, IA, X, IK>, PT>,
{
}

/// The key of the locator `L`, `Self`, is what tells its candidates apart,
/// `K`.
#[diagnostic::on_unimplemented(
    message = "`{L}` picks by keys of type `{Self}`, but its candidates are told apart by `{K}`",
    label = "the locator's key is not that of its candidates",
    note = "`Locator::Key` is the key of the members of the `Under` of `Locator::Among`: the keys of the levels between the anchor and the candidates, a tuple for more than one, `()` for none"
)]
pub trait PicksBy<L, K> {}
impl<L, K> PicksBy<L, K> for K {}

/// Every taking of a list only reads.
pub trait OnlyReads {}
impl OnlyReads for HNil {}
impl<Rt, X, E: InputReads<X>, T: OnlyReads> OnlyReads for HCons<Touch<Rt, X, E>, T> {}
impl<I, Sd, W, T: OnlyReads> OnlyReads for HCons<Deep<I, Sd, W>, T> {}
/// A taking by the input of a locator of `X` that only reads.
#[diagnostic::on_unimplemented(
    message = "the input of a locator takes `{X}` exclusive; it may only read",
    label = "the input takes exclusive",
    note = "`Locator::Input` is lent to pick the node, beside what is taken there: it only reads, taking `Ref`, `OwnKey`, `OwnAddress`, `Clock` and `WakerOf`, never `Mut`"
)]
pub trait InputReads<X> {}
impl<X> InputReads<X> for False {}

impl<C, L, A, X, T, IA, IK, Sh, PI, PT> Takes<C, LocPos<IA, IK, Sh, PI, PT>> for At<L, T>
where
    C: Ctx,
    L: Locator<Among = Under<A, X>>,
    L::Input: Takes<C, PI>,
    C::Chain: FindUp<A, IA>,
    UpKind<C, A, IA>: FindBelow<C::Parts, X, IK, Sh = Sh>,
    UpPre<C, A, IA>: Append<<UpKind<C, A, IA> as FindBelow<C::Parts, X, IK>>::Rel>,
    LocCx<C, A, IA, X, IK>: Ctx,
    T: Takes<LocCx<C, A, IA, X, IK>, PT> + TakesExclusive,
    T::Out: DeepIf<Deep<IA, C::Depth, Self>>,
    <L::Input as Takes<C, PI>>::Touches: Append<T::Touches>,
    <T::Out as DeepIf<Deep<IA, C::Depth, Self>>>::Out:
        Append<<<L::Input as Takes<C, PI>>::Touches as Append<T::Touches>>::Out>,
{
    /// The anchor's depth, when what is taken at the node picked is
    /// exclusive, since the candidates below an anchor above the members
    /// are not all in one member; what the input and the need at the node
    /// picked take.
    type Touches = <<T::Out as DeepIf<Deep<IA, C::Depth, Self>>>::Out as Append<
        <<L::Input as Takes<C, PI>>::Touches as Append<T::Touches>>::Out,
    >>::Out;
}

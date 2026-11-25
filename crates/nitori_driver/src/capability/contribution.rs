//! Where a part's contribution to a family is written, and the pieces of
//! every part put together: the part's own list and the composition's,
//! searched by the numbers of the part and the family.
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

use super::Contribute;
use crate::{
    call::{__pack::AnyIs, Composition},
    component::Part,
    id::{False, NumberEq, Numbered, True},
    need::{Need, Pos, View},
};

/// An item of a part's [`Contributions`](Part::Contributions): the
/// part's contribution `C` to the family `F`, `C` implementing
/// [`Contribute<F>`](Contribute).
///
/// The family is found by its number ([`Numbered`]).
pub struct Contribution<F, C>(PhantomData<fn() -> (F, C)>);

/// An item of a composition's
/// [`Contributions`](Composition::Contributions): the contribution `C`
/// of the part `O` to the family `F`, `C` implementing
/// [`Contribute<F>`](Contribute).
///
/// It is written for a part that does not know the family. An item of the
/// composition's list may also be a whole list of these, spliced in: what
/// the crate of a family exports for the parts it knows. The part and the
/// family are found by their numbers ([`Numbered`]); a part that is not
/// one of the composition's does not compile.
#[allow(clippy::type_complexity)]
pub struct PartContribution<O, F, C>(PhantomData<fn() -> (O, F, C)>);

/// The lists the pieces of a composition are found in: its parts and its
/// own list of contributions. Every [`Composition`] has them; not for use
/// otherwise.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a composition",
    note = "`Contributed<F, Q>` takes the composition `Q` the need is lent under, whose parts and list of contributions give every part's piece"
)]
pub trait PieceLists: 'static {
    /// The parts.
    type Parts;
    /// The composition's own list of contributions.
    type Contributions;
}
impl<Q: Composition> PieceLists for Q {
    type Parts = Q::Parts;
    type Contributions = Q::Contributions;
}

/// The pieces of the composition `Self` for the family `F`, one per
/// part, in the order of the parts.
///
/// Every composition has them where every part's piece is written once or
/// not at all; code generic over the composition bounds it, saying what
/// it does with the pieces.
pub trait PieceList<F>: 'static {
    /// Every part's needs, one list per part.
    type Needs: Need;
    /// Every part's piece.
    type Pieces<'t>;
    /// Put every piece together from the view of every part's needs.
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Pieces<'t>;
}
impl<F, Q: PieceLists> PieceList<F> for Q
where
    Q::Parts: PartPieces<F, Q>,
{
    type Needs = <Q::Parts as PartPieces<F, Q>>::Needs;
    type Pieces<'t> = <Q::Parts as PartPieces<F, Q>>::Pieces<'t>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Pieces<'t> {
        <Q::Parts as PartPieces<F, Q>>::assemble::<P>(view)
    }
}
/// The pieces of the composition `Q` for the family `F`, lent for `'t`.
pub type PiecesOf<'t, Q, F> = <Q as PieceList<F>>::Pieces<'t>;

/// The pieces of a list of parts `Self` of the composition `Q` for the
/// family `F`.
#[doc(hidden)]
pub trait PartPieces<F, Q> {
    /// Every part's needs, one list per part.
    type Needs: Need;
    /// Every part's piece.
    type Pieces<'t>;
    /// Put every piece together.
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Pieces<'t>;
}
impl<F, Q> PartPieces<F, Q> for HNil {
    type Needs = HNil;
    type Pieces<'t> = HNil;
    fn assemble<'t, P: Pos>(_: View<'t, Self::Needs, P>) -> Self::Pieces<'t> {
        HNil
    }
}
impl<F, Q, O: ContributionTo<F, Q>, T: PartPieces<F, Q>> PartPieces<F, Q> for HCons<O, T> {
    type Needs = HCons<<O::Contribution as Contribute<F>>::Needs, T::Needs>;
    type Pieces<'t> = HCons<<O::Contribution as Contribute<F>>::Piece<'t>, T::Pieces<'t>>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Pieces<'t> {
        HCons {
            head: <O::Contribution as Contribute<F>>::assemble::<P::Head>(view.head),
            tail: T::assemble::<P::Tail>(view.tail),
        }
    }
}

/// The contribution of the part `Self` of the composition `Q` to the
/// family `F`: the one written in its own list or in the composition's,
/// or `()`.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "the piece of the part `{Self}` for the family `{F}` is not found in `{Q}`",
    note = "a part's piece is found by the numbers of the part and the family: the family is `#[derive(Numbered)]`, and so is every part of the composition"
)]
pub trait ContributionTo<F, Q> {
    /// The contribution.
    type Contribution: Contribute<F>;
}
impl<F, Q: PieceLists, O: Part<Q::Parts>> ContributionTo<F, Q> for O
where
    O::Contributions: FindOwn<F>,
    Q::Contributions: FindListed<O, F>,
    <O::Contributions as FindOwn<F>>::Out:
        Settle<O, F, <Q::Contributions as FindListed<O, F>>::Out>,
{
    type Contribution = <<O::Contributions as FindOwn<F>>::Out as Settle<
        O,
        F,
        <Q::Contributions as FindListed<O, F>>::Out,
    >>::Out;
}

/// A search found nothing.
#[doc(hidden)]
pub struct Unlisted;
/// A search found the contribution `C`, once.
#[doc(hidden)]
pub struct Listed<C>(PhantomData<fn() -> C>);
/// A search found more than one.
#[doc(hidden)]
pub struct ListedTwice;

/// The item at the family `F` of a part's own list `Self`, by the
/// families' numbers.
#[doc(hidden)]
pub trait FindOwn<F> {
    /// [`Unlisted`], [`Listed`] or [`ListedTwice`].
    type Out;
}
impl<F> FindOwn<F> for HNil {
    type Out = Unlisted;
}
impl<F: Numbered, G: Numbered, C, T: FindOwn<F>> FindOwn<F> for HCons<Contribution<G, C>, T>
where
    G::Number: NumberEq<F::Number>,
    <G::Number as NumberEq<F::Number>>::Out: Hit<C, T::Out>,
{
    type Out = <<G::Number as NumberEq<F::Number>>::Out as Hit<C, T::Out>>::Out;
}

/// The item at the part `O` and the family `F` of a composition's list
/// `Self`, by their numbers; an item that is a list is searched in turn.
#[doc(hidden)]
pub trait FindListed<O, F> {
    /// [`Unlisted`], [`Listed`] or [`ListedTwice`].
    type Out;
}
impl<O, F> FindListed<O, F> for HNil {
    type Out = Unlisted;
}
impl<O: Numbered, F: Numbered, P: Numbered, G: Numbered, C, T> FindListed<O, F>
    for HCons<PartContribution<P, G, C>, T>
where
    T: FindListed<O, F>,
    P::Number: NumberEq<O::Number>,
    G::Number: NumberEq<F::Number>,
    <P::Number as NumberEq<O::Number>>::Out: Both<<G::Number as NumberEq<F::Number>>::Out>,
    <<P::Number as NumberEq<O::Number>>::Out as Both<<G::Number as NumberEq<F::Number>>::Out>>::Out:
        Hit<C, T::Out>,
{
    type Out = <<<P::Number as NumberEq<O::Number>>::Out as Both<
        <G::Number as NumberEq<F::Number>>::Out,
    >>::Out as Hit<C, T::Out>>::Out;
}
/// A list spliced in: searched, then the items after it.
impl<O, F, H, HT, T> FindListed<O, F> for HCons<HCons<H, HT>, T>
where
    HCons<H, HT>: FindListed<O, F>,
    T: FindListed<O, F>,
    <HCons<H, HT> as FindListed<O, F>>::Out: Join<T::Out>,
{
    type Out = <<HCons<H, HT> as FindListed<O, F>>::Out as Join<T::Out>>::Out;
}
/// An empty list spliced in.
impl<O, F, T: FindListed<O, F>> FindListed<O, F> for HCons<HNil, T> {
    type Out = T::Out;
}

/// Both answers yes.
#[doc(hidden)]
pub trait Both<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl<B> Both<B> for True {
    type Out = B;
}
impl<B> Both<B> for False {
    type Out = False;
}
/// An item that matched (`Self` is [`True`]) or did not, with what the
/// items after it found.
#[doc(hidden)]
pub trait Hit<C, Rest> {
    /// [`Unlisted`], [`Listed`] or [`ListedTwice`].
    type Out;
}
impl<C, Rest> Hit<C, Rest> for False {
    type Out = Rest;
}
impl<C> Hit<C, Unlisted> for True {
    type Out = Listed<C>;
}
impl<C, D> Hit<C, Listed<D>> for True {
    type Out = ListedTwice;
}
impl<C> Hit<C, ListedTwice> for True {
    type Out = ListedTwice;
}
/// What a list spliced in found, with what the items after it found.
#[doc(hidden)]
pub trait Join<B> {
    /// [`Unlisted`], [`Listed`] or [`ListedTwice`].
    type Out;
}
impl<B> Join<B> for Unlisted {
    type Out = B;
}
impl<C> Join<Unlisted> for Listed<C> {
    type Out = Listed<C>;
}
impl<C, D> Join<Listed<D>> for Listed<C> {
    type Out = ListedTwice;
}
impl<C> Join<ListedTwice> for Listed<C> {
    type Out = ListedTwice;
}
impl<B> Join<B> for ListedTwice {
    type Out = ListedTwice;
}

/// What the part's own list found (`Self`) and what the composition's
/// found (`G`), settled: the one contribution, or `()` when neither has
/// one.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "the piece of the part `{O}` for the family `{F}` is written in more than one place",
    note = "a part's contribution to a family is written once: in the part's own `Contributions` or in the composition's, not both, and once in each list, the lists spliced into the composition's counted"
)]
pub trait Settle<O, F, G> {
    /// The contribution.
    type Out: Contribute<F>;
}
impl<O, F> Settle<O, F, Unlisted> for Unlisted {
    type Out = ();
}
impl<O, F, C: Contribute<F>> Settle<O, F, Unlisted> for Listed<C> {
    type Out = C;
}
impl<O, F, C: Contribute<F>> Settle<O, F, Listed<C>> for Unlisted {
    type Out = C;
}

/// Every item of a composition's list `Self` names one of its parts
/// `Parts`, by number, and is the one place the piece of that part for
/// that family is written: not in the part's own list as well, nor twice
/// in `All`, the composition's whole list; the lists spliced in too.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a list of contributions",
    note = "a composition's `Contributions` lists `PartContribution<O, F, C>`, or whole lists of them, spliced in",
    note = "the parts and the families are found by their numbers: every part of the composition, and every family it lists, is `#[derive(Numbered)]`"
)]
pub trait ListedKnown<Parts, All> {}
impl<Parts, All> ListedKnown<Parts, All> for HNil {}
impl<Parts, All, O, F, C, T> ListedKnown<Parts, All> for HCons<PartContribution<O, F, C>, T>
where
    Parts: AnyIs<O>,
    <Parts as AnyIs<O>>::Out: PartOfComposition<O> + ListedOnce<O, F, Parts, All>,
    T: ListedKnown<Parts, All>,
{
}
impl<Parts, All, H, HT, T> ListedKnown<Parts, All> for HCons<HCons<H, HT>, T>
where
    HCons<H, HT>: ListedKnown<Parts, All>,
    T: ListedKnown<Parts, All>,
{
}
impl<Parts, All, T: ListedKnown<Parts, All>> ListedKnown<Parts, All> for HCons<HNil, T> {}

/// The piece of the part `O` for the family `F`, listed in the
/// composition's whole list `All`, is written there once and not in the
/// part's own list: checked when `O` is one of the parts `Parts` (`Self`
/// is [`True`]); when it is not, [`PartOfComposition`] says so.
#[doc(hidden)]
pub trait ListedOnce<O, F, Parts, All> {}
impl<O, F, Parts, All> ListedOnce<O, F, Parts, All> for True
where
    O: Part<Parts>,
    O::Contributions: FindOwn<F>,
    All: FindListed<O, F>,
    <O::Contributions as FindOwn<F>>::Out: WrittenOnce<O, F, <All as FindListed<O, F>>::Out>,
{
}
impl<O, F, Parts, All> ListedOnce<O, F, Parts, All> for False {}

/// Every part of the list `Self`, in a composition of the parts `Parts`,
/// lists each family once in its own list.
#[doc(hidden)]
pub trait OwnListedOnce<Parts> {}
impl<Parts> OwnListedOnce<Parts> for HNil {}
impl<Parts, O, Os> OwnListedOnce<Parts> for HCons<O, Os>
where
    O: Part<Parts>,
    O::Contributions: OwnOnce<O, O::Contributions>,
    Os: OwnListedOnce<Parts>,
{
}
/// Every family of the items `Self` of the part `O`'s own list `All` is
/// in `All` once.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a list of contributions of the part `{O}`",
    note = "a part's `Contributions` lists `Contribution<F, C>`, one per family; every family it lists is `#[derive(Numbered)]`"
)]
pub trait OwnOnce<O, All> {}
impl<O, All> OwnOnce<O, All> for HNil {}
impl<O, All, G, C, T> OwnOnce<O, All> for HCons<Contribution<G, C>, T>
where
    All: FindOwn<G>,
    <All as FindOwn<G>>::Out: WrittenOnce<O, G, Unlisted>,
    T: OwnOnce<O, All>,
{
}

/// What the part's own list found (`Self`) and what the composition's
/// found (`G`) are one item between them: checked where the composition
/// is written, whether or not a need takes the family.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "the piece of the part `{O}` for the family `{F}` is written in more than one place",
    note = "a part's contribution to a family is written once: in the part's own `Contributions` or in the composition's, not both, and once in each list, the lists spliced into the composition's counted"
)]
pub trait WrittenOnce<O, F, G> {}
impl<O, F, C> WrittenOnce<O, F, Listed<C>> for Unlisted {}
impl<O, F, C> WrittenOnce<O, F, Unlisted> for Listed<C> {}
/// The part `O` a composition's list names is one of its parts: [`True`]
/// only.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "the composition lists a piece of `{O}`, which is not one of its parts",
    label = "`{O}` is not among the parts",
    note = "an item `PartContribution<O, F, C>` of a composition's `Contributions`, or of a list spliced in, names a part of the composition; a contribution of a part it does not have is left out of the list"
)]
pub trait PartOfComposition<O> {}
impl<O> PartOfComposition<O> for True {}

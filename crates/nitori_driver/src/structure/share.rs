//! The parts' shares: what a part lists for the kinds of nodes it extends,
//! and the share of a part at a kind, found by the kind's number.
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HList, HNil};

use crate::{
    call::__pack::Or,
    component::Part,
    id::{False, NumberEq, Numbered, True},
};

/// The share a part keeps in each node of the kind `K`: an item of its
/// [`Part::Shares`], one per kind it extends. On a kind it does not list,
/// the part keeps nothing: no components, no calls and no reactions.
///
/// - `Data`: the components, a list (`HNil` for none).
/// - `Calls`: the kinds of the calls the part keeps at each node of this
///   kind, an `HList`; none by default. A new node's calls start after its
///   components are built, in the order of the parts, then in this order
///   ([`CallKind::start`](crate::call::CallKind::start)).
/// - `Reacts`: the reactions of the part at the nodes of this kind: a list
///   of [`ReactsTo`](crate::react::ReactsTo)`<point, reaction>`, none by
///   default.
///
/// The kind is found by its number ([`Numbered`]): a kind of a part's
/// list is `#[derive(Numbered)]`. A part that lists a kind of the
/// structure twice does not compile. A share at a kind the structure does
/// not have is left out, not refused, and such a kind listed twice is not
/// checked either: every item at it is left out. So one part can be
/// composed in structures of different kinds; listing a kind of another
/// structure by mistake keeps
/// that share nowhere, which a need taking one of its components then
/// fails to find, while its calls and reactions are just not there.
#[allow(clippy::type_complexity)]
pub struct Share<K, Data, Calls = HNil, Reacts = HNil>(
    PhantomData<fn() -> (K, Data, Calls, Reacts)>,
);

/// The share of the part `Self` at the kind `K` in a composition of the
/// parts `L`: the item of its list at `K`, or nothing when it lists none.
#[doc(hidden)]
pub trait ShareAt<K, L> {
    /// The components.
    type Data: HList + 'static;
    /// The kinds of the calls.
    type Calls: HList + 'static;
    /// The reactions.
    type Reacts: 'static;
}
impl<O: Part<L>, K, L> ShareAt<K, L> for O
where
    O::Shares: FindShare<O, K>,
{
    type Data = <O::Shares as FindShare<O, K>>::Data;
    type Calls = <O::Shares as FindShare<O, K>>::Calls;
    type Reacts = <O::Shares as FindShare<O, K>>::Reacts;
}

/// The item at the kind `K` of a list `Self` of the part `O`'s shares,
/// by the kinds' numbers; nothing when no item is at `K`.
#[doc(hidden)]
pub trait FindShare<O, K> {
    /// The components.
    type Data: HList + 'static;
    /// The kinds of the calls.
    type Calls: HList + 'static;
    /// The reactions.
    type Reacts: 'static;
}
impl<O, K> FindShare<O, K> for HNil {
    type Data = HNil;
    type Calls = HNil;
    type Reacts = HNil;
}
impl<O, K: Numbered, J: Numbered, D, C, R, T> FindShare<O, K> for HCons<Share<J, D, C, R>, T>
where
    J::Number: NumberEq<K::Number>,
    <J::Number as NumberEq<K::Number>>::Out: PickShare<O, K, Share<J, D, C, R>, T>,
{
    type Data =
        <<J::Number as NumberEq<K::Number>>::Out as PickShare<O, K, Share<J, D, C, R>, T>>::Data;
    type Calls =
        <<J::Number as NumberEq<K::Number>>::Out as PickShare<O, K, Share<J, D, C, R>, T>>::Calls;
    type Reacts =
        <<J::Number as NumberEq<K::Number>>::Out as PickShare<O, K, Share<J, D, C, R>, T>>::Reacts;
}
/// The item `S` (`Self` is [`True`]: it is at `K`, and none of the
/// items `T` after it is), or the item at `K` among `T`.
#[doc(hidden)]
pub trait PickShare<O, K, S, T> {
    /// The components.
    type Data: HList + 'static;
    /// The kinds of the calls.
    type Calls: HList + 'static;
    /// The reactions.
    type Reacts: 'static;
}
impl<O, K, J, D, C, R, T> PickShare<O, K, Share<J, D, C, R>, T> for True
where
    D: HList + 'static,
    C: HList + 'static,
    R: 'static,
    T: ShareListed<K>,
    T::Out: OneShare<O, K>,
{
    type Data = D;
    type Calls = C;
    type Reacts = R;
}
impl<O, K, S, T: FindShare<O, K>> PickShare<O, K, S, T> for False {
    type Data = T::Data;
    type Calls = T::Calls;
    type Reacts = T::Reacts;
}

/// Whether a list `Self` of shares has an item at the kind `K`: [`True`]
/// or [`False`].
#[doc(hidden)]
pub trait ShareListed<K> {
    /// The answer.
    type Out;
}
impl<K> ShareListed<K> for HNil {
    type Out = False;
}
impl<K: Numbered, J: Numbered, D, C, R, T: ShareListed<K>> ShareListed<K>
    for HCons<Share<J, D, C, R>, T>
where
    J::Number: NumberEq<K::Number>,
    <J::Number as NumberEq<K::Number>>::Out: Or<T::Out>,
{
    type Out = <<J::Number as NumberEq<K::Number>>::Out as Or<T::Out>>::Out;
}
/// No second share of the part `O` at the kind `K`: [`False`] only.
#[diagnostic::on_unimplemented(
    message = "the part `{O}` lists two shares at `{K}`",
    label = "a second share at `{K}`",
    note = "a part lists one `Share` per kind it extends: put the components, calls and reactions of `{K}` in one item"
)]
pub trait OneShare<O, K> {}
impl<O, K> OneShare<O, K> for False {}

/// The shares of a list of parts `Self` in a node of the kind `K`, one
/// list per part, in the order of the parts; `Self` is the whole list of
/// the composition.
#[doc(hidden)]
pub trait Shares<K>: 'static {
    /// The list of the shares.
    type Data: 'static;
}
impl<K, L: SharesIn<K, L> + 'static> Shares<K> for L {
    type Data = <L as SharesIn<K, L>>::Data;
}
/// The shares at the kind `K` of the parts `Self`, a part of the list
/// `L`.
#[doc(hidden)]
pub trait SharesIn<K, L> {
    /// The list of the shares.
    type Data: 'static;
}
impl<K, L> SharesIn<K, L> for HNil {
    type Data = HNil;
}
impl<K, L, O: ShareAt<K, L> + 'static, Os: SharesIn<K, L>> SharesIn<K, L> for HCons<O, Os> {
    type Data = HCons<O::Data, Os::Data>;
}

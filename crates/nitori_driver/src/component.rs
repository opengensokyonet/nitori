//! What the driver does with every component of a node: its operations.
//!
//! A component is the `Self` of [`Component<O, L, K>`](Component): `O` is
//! the part whose share it is in, `L` the whole list of parts, `K` the
//! node kind. A component may be generic over the list, and act
//! differently by which parts are in it. The part `O` comes before `L`,
//! so the crate that defines a part can write the operations of a generic
//! type of another crate (`impl<X, L, K> Component<MyPart, L, K> for
//! TheirType<X>`). A bare type parameter the host chooses cannot be `Self`
//! of a foreign trait (E0210); such a component is kept in [`Held<X>`].
//!
//! The operations are associated functions, synchronous, which the driver
//! calls: [`build`](Component::build) when the node is added,
//! [`wake`](Component::wake) when the component's waker is woken,
//! [`end`](Component::end) when the node ends. Each is lent a view, as a
//! call is: `wake` and `end` the component itself, exclusive, then its
//! [`Needs`](Component::Needs) at its node ([`ComponentView`]); `build`
//! its [`BuildNeeds`](Component::BuildNeeds), taken at the new node from
//! the nodes above it, which are there already: its own key is
//! [`OwnKey`](crate::need::OwnKey), the components of the parent and above
//! are [`Mut`](crate::need::Mut) and [`Ref`](crate::need::Ref). The needs
//! are proved as a call's are, and do not compile for the same reasons.
//!
//! A part lists the kinds of nodes it extends, one
//! [`Share`](crate::structure::Share) each, in [`Part::Shares`]: the
//! components there, the calls and the reactions. In a kind it does not
//! list, it keeps nothing. Its contributions to the families of contributed
//! capabilities are listed in [`Part::Contributions`], one
//! [`Contribution`](crate::capability::Contribution) each
//! ([`capability`](crate::capability)).
//!
//! Every part has construction arguments, whose type may depend on the
//! whole list ([`Part<L>`](Part)); the driver puts them in one list
//! ([`ArgsOf<L>`](ArgsOf)). Every `build` is handed the arguments of its
//! own part, and all of the list, in which [`part_args`] finds another
//! part's by its number, or nothing when the list does not have it:
//!
//! ```
//! use nitori_driver::{
//!     component::{ArgsOf, Component, FindArgs, Part, PartList, part_args},
//!     frunk_core::{HList, hlist::HNil, hlist},
//!     id::Numbered,
//!     need::{EmptyPos, Pos, View},
//!     structure::NodeKind,
//! };
//!
//! struct Root;
//! impl NodeKind for Root {
//!     type Children = HNil;
//! }
//! /// Two parts and their arguments.
//! #[derive(Numbered)]
//! struct Base;
//! struct BaseArgs {
//!     window: u64,
//! }
//! impl<L> Part<L> for Base {
//!     type Args = BaseArgs;
//! }
//! #[derive(Numbered)]
//! struct Reset;
//! struct ResetArgs {
//!     after: u64,
//! }
//! impl<L> Part<L> for Reset {
//!     type Args = ResetArgs;
//! }
//!
//! /// A component of `Base`, generic over the list of parts: it resets
//! /// its streams only when the list has `Reset`.
//! struct Streams {
//!     window: u64,
//!     resets: Option<u64>,
//! }
//! impl<L, K: NodeKind> Component<Base, L, K> for Streams
//! where
//!     L: PartList<L> + FindArgs<Reset, L>,
//! {
//!     fn build<P: Pos>(
//!         _: &mut K::Start,
//!         own: &BaseArgs,
//!         args: &ArgsOf<L>,
//!         _: View<'_, HNil, P>,
//!     ) -> Self {
//!         let resets = part_args::<Reset, L>(args).map(|r| r.after);
//!         Streams { window: own.window, resets }
//!     }
//! }
//!
//! type With = HList![Base, Reset];
//! let s = <Streams as Component<Base, With, Root>>::build::<EmptyPos>(
//!     &mut (),
//!     &BaseArgs { window: 8 },
//!     &hlist![BaseArgs { window: 8 }, ResetArgs { after: 3 }],
//!     HNil,
//! );
//! assert_eq!((s.window, s.resets), (8, Some(3)));
//! type Without = HList![Base];
//! let s = <Streams as Component<Base, Without, Root>>::build::<EmptyPos>(
//!     &mut (),
//!     &BaseArgs { window: 8 },
//!     &hlist![BaseArgs { window: 8 }],
//!     HNil,
//! );
//! assert_eq!((s.window, s.resets), (8, None));
//! ```
//!
//! A host's bare type is not a component of another crate's part:
//!
//! ```compile_fail,E0210
//! use nitori_driver::{
//!     component::{ArgsOf, Component, Part, PartList},
//!     frunk_core::hlist::HNil,
//!     id::Numbered,
//!     need::{Pos, View},
//!     structure::NodeKind,
//! };
//! trait Estimator {
//!     fn new() -> Self;
//! }
//! #[derive(Numbered)]
//! struct Base;
//! impl<L> Part<L> for Base {
//!     type Args = ();
//! }
//! impl<X: Estimator + 'static, L: PartList<L>, K: NodeKind> Component<Base, L, K> for X {
//!     fn build<P: Pos>(_: &mut K::Start, _: &(), _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
//!         X::new()
//!     }
//! }
//! ```
//!
//! it is kept in [`Held`]:
//!
//! ```
//! use nitori_driver::{
//!     component::{ArgsOf, Component, Held, Part, PartList},
//!     frunk_core::hlist::HNil,
//!     id::Numbered,
//!     need::{Pos, View},
//!     structure::NodeKind,
//! };
//! trait Estimator {
//!     fn new() -> Self;
//! }
//! #[derive(Numbered)]
//! struct Base;
//! impl<L> Part<L> for Base {
//!     type Args = ();
//! }
//! impl<X, L, K> Component<Base, L, K> for Held<X>
//! where
//!     X: Estimator + Numbered + 'static,
//!     L: PartList<L>,
//!     K: NodeKind,
//! {
//!     fn build<P: Pos>(_: &mut K::Start, _: &(), _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
//!         Held(X::new())
//!     }
//! }
//! ```
//!
//! The needs of a component's operations do not overlap the component
//! itself, which the view holds exclusive: a link core of the doc
//! structure that also asks for itself does not compile.
//!
//! ```compile_fail,E0277
//! # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
//! component_lends!(LinkPath, LinkCore, HList![Ref<LinkCore>]);
//! ```
//!
//! ```
//! # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
//! component_lends!(LinkPath, LinkCore, HList![Ref<Planner>, Ref<ConnCore>]);
//! ```
use frunk_core::hlist::{HCons, HNil};

use crate::{
    id::{False, NumberEq, Numbered, True},
    need::{Need, Pos, View},
    structure::NodeKind,
};

/// The construction arguments of the part `Self` in a composition of the
/// parts `L`; their type may depend on the whole list.
///
/// Every part of a composition is `#[derive(Numbered)]` ([`Numbered`]),
/// checked where the composition is written: its contributions and its
/// arguments ([`part_args`]) are found by its number. A part without one
/// does not compile, whatever it lists:
///
/// ```compile_fail,E0277
/// #![feature(impl_trait_in_assoc_type)]
/// use nitori_driver::{
///     __private::calls::{Count, Item, Root, StillClock, Total},
///     component::Part,
///     composition,
///     frunk_core::HList,
///     structure::Share,
/// };
/// struct Counter;
/// impl<L> Part<L> for Counter {
///     type Args = ();
///     type Shares = HList![Share<Root, HList![Total]>, Share<Item, HList![Count]>];
/// }
/// struct Q;
/// composition!(impl for Q: Root, HList![Counter], StillClock, u8);
/// ```
pub trait Part<L> {
    /// The arguments.
    type Args;
    /// The shares of this part in the kinds of nodes it extends, one
    /// [`Share`](crate::structure::Share) per kind; none by default. On a
    /// kind it does not list, the part keeps nothing: no components, no
    /// calls and no reactions. A kind of the structure listed twice does
    /// not compile; the shares at a kind the structure it is composed in
    /// does not have are left out, not refused, and not checked for being
    /// listed twice.
    type Shares = HNil;
    /// The part's contributions to the families of contributed
    /// capabilities, one [`Contribution`](crate::capability::Contribution)
    /// per family; none by default. To a family it does not list, and the
    /// composition's [`Contributions`](crate::call::Composition::Contributions)
    /// does not list for it either, it contributes `()`. See
    /// [`capability`](crate::capability).
    type Contributions = HNil;
}
/// The arguments of a list of parts `Self` in a composition of the parts
/// `L`: one per part, in order.
pub trait PartList<L> {
    /// The list of arguments.
    type Args;
}
impl<L> PartList<L> for HNil {
    type Args = HNil;
}
impl<L, P: Part<L>, Ps: PartList<L>> PartList<L> for HCons<P, Ps> {
    type Args = HCons<P::Args, Ps::Args>;
}
/// The arguments of every part of the list `L`.
pub type ArgsOf<L> = <L as PartList<L>>::Args;

/// The arguments of the part `O` in `args`, those of every part of the
/// list `L`, found by the part's number; `None` when `L` does not have
/// `O`. What a component's [`build`](Component::build) reads another
/// part's arguments with; its own it is handed. Every part of `L`, and
/// `O`, is `#[derive(Numbered)]` ([`Numbered`]), whichever part is
/// looked for and wherever it is in the list.
pub fn part_args<O, L>(args: &ArgsOf<L>) -> Option<&<O as Part<L>>::Args>
where
    O: Part<L>,
    L: FindArgs<O, L>,
{
    <L as FindArgs<O, L>>::find(args)
}

/// Every part of the list `Self` has a number ([`Numbered`]): what the
/// proof of a composition asks of its parts.
#[doc(hidden)]
pub trait PartsNumbered {}
impl PartsNumbered for HNil {}
impl<P: Numbered, Ps: PartsNumbered> PartsNumbered for HCons<P, Ps> {}

/// The arguments of the part `O` in those of a list `Self` of parts, in a
/// composition of the parts `L`, by the parts' numbers: the first part of
/// the list with the number of `O`, or none. What [`part_args`] finds
/// with; every part of the list, and `O`, has a number
/// ([`Numbered`]).
#[diagnostic::on_unimplemented(
    message = "the arguments of `{O}` are not looked for in those of the parts `{Self}`",
    label = "a list of parts with a number each",
    note = "`part_args` finds a part's arguments by the parts' numbers: every part of the list, and `{O}`, is `#[derive(Numbered)]`; a component generic over the list of parts bounds it by `FindArgs<{O}, L>`"
)]
pub trait FindArgs<O: Part<L>, L>: PartList<L> {
    /// The arguments of `O` in `args`, if the list has it.
    fn find(args: &Self::Args) -> Option<&O::Args>;
}
impl<O: Part<L>, L> FindArgs<O, L> for HNil {
    fn find(_: &HNil) -> Option<&O::Args> {
        None
    }
}
impl<O, L, P, Ps> FindArgs<O, L> for HCons<P, Ps>
where
    O: Part<L> + Numbered,
    P: Part<L> + Numbered,
    Ps: FindArgs<O, L>,
    P::Number: NumberEq<O::Number>,
    <P::Number as NumberEq<O::Number>>::Out: PickArgs<O, L, P, Ps>,
{
    fn find(args: &Self::Args) -> Option<&O::Args> {
        <<P::Number as NumberEq<O::Number>>::Out as PickArgs<O, L, P, Ps>>::pick(args)
    }
}
/// The arguments of `O` in those of the parts `HCons<P, Ps>`, by whether
/// `P` has the number of `O` (`Self`: [`True`] or [`False`]).
#[doc(hidden)]
pub trait PickArgs<O: Part<L>, L, P: Part<L>, Ps: PartList<L>> {
    /// The head's arguments, or what the tail has.
    fn pick(args: &HCons<P::Args, Ps::Args>) -> Option<&O::Args>;
}
impl<O: Part<L>, L, P: Part<L, Args = O::Args>, Ps: PartList<L>> PickArgs<O, L, P, Ps> for True {
    fn pick(args: &HCons<P::Args, Ps::Args>) -> Option<&O::Args> {
        Some(&args.head)
    }
}
impl<O: Part<L>, L, P: Part<L>, Ps: FindArgs<O, L>> PickArgs<O, L, P, Ps> for False {
    fn pick(args: &HCons<P::Args, Ps::Args>) -> Option<&O::Args> {
        Ps::find(&args.tail)
    }
}

/// The view a component's [`wake`](Component::wake) and
/// [`end`](Component::end) are lent for `'t`: the component `C`,
/// exclusive, then the view of its needs `N` at the position `P`.
pub type ComponentView<'t, C, N, P> = HCons<&'t mut C, View<'t, N, P>>;

/// Why a node ends, as its components' [`end`](Component::end) is told;
/// `R` is its kind's reason ([`NodeKind::EndReason`]).
#[derive(Debug, PartialEq, Eq)]
pub enum EndCause<'a, R> {
    /// Ended for this reason, which the request to end it gave.
    Reason(&'a R),
    /// A node above it ended, and it ends with it.
    Above,
    /// The driver was dropped before the node ended.
    Dropped,
}
impl<R> Clone for EndCause<'_, R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R> Copy for EndCause<'_, R> {}

/// The operations of a component in the share of the part `O`, in a
/// composition of the parts `L`, in a node of the kind `K`.
///
/// [`wake`](Component::wake) and [`end`](Component::end) do nothing by
/// default. Each operation is generic over the position `P` of its needs,
/// as a call is over the structure: the driver gives the position it
/// inferred where the structure is known.
pub trait Component<O: Part<L>, L: PartList<L>, K: NodeKind>: Sized + 'static {
    /// What [`wake`](Component::wake) and [`end`](Component::end) take
    /// around the node besides the component: needs, lent at the node;
    /// none by default.
    type Needs: Need = HNil;
    /// What [`build`](Component::build) takes: needs, lent at the new node
    /// from the nodes above it; none by default. The root's components
    /// have none.
    type BuildNeeds: Need = HNil;
    /// The component of a new node, from what the one adding the node gave
    /// it to start from, the arguments of its part `own`, those of every
    /// part `args` (another part's are found with [`part_args`]) and the
    /// view of its build needs. The node's components are built in the
    /// order of the parts, each handed the same `start`, and may take out
    /// what they keep of it.
    fn build<P: Pos>(
        start: &mut K::Start,
        own: &<O as Part<L>>::Args,
        args: &ArgsOf<L>,
        view: View<'_, Self::BuildNeeds, P>,
    ) -> Self;
    /// The component's waker was woken.
    fn wake<P: Pos>(view: ComponentView<'_, Self, Self::Needs, P>) {
        let _ = view;
    }
    /// The node ends, for `cause`: the nodes below it ended, and it is
    /// still in the tree; the component is dropped afterwards.
    fn end<P: Pos>(
        view: ComponentView<'_, Self, Self::Needs, P>,
        cause: EndCause<'_, K::EndReason>,
    ) {
        let _ = (view, cause);
    }
}

/// A component whose type is a type parameter the host chooses, kept so
/// that a part can write its operations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Numbered)]
pub struct Held<X>(pub X);

#[cfg(test)]
mod tests;

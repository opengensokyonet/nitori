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
//! list, it keeps nothing.
//!
//! Every part has construction arguments, whose type may depend on the
//! whole list ([`Part<L>`](Part)); the driver puts them in one list
//! ([`ArgsOf<L>`](ArgsOf)) and gives every `build` all of it. A component
//! reads what it needs through a trait of its own crate on that list,
//! answered part by part:
//!
//! ```
//! use nitori_driver::{
//!     component::{ArgsOf, Component, Part, PartList},
//!     frunk_core::{HList, hlist::{HCons, HNil}, hlist},
//!     need::{EmptyPos, Pos, View},
//!     structure::NodeKind,
//! };
//!
//! struct Root;
//! impl NodeKind for Root {
//!     type Children = HNil;
//! }
//! /// Two parts and their arguments.
//! struct Base;
//! struct BaseArgs {
//!     window: u64,
//! }
//! impl<L> Part<L> for Base {
//!     type Args = BaseArgs;
//! }
//! struct Reset;
//! struct ResetArgs;
//! impl<L> Part<L> for Reset {
//!     type Args = ResetArgs;
//! }
//! /// What a set of streams reads of one part's arguments.
//! trait StreamArgs {
//!     fn window(&self) -> Option<u64> {
//!         None
//!     }
//!     fn resets(&self) -> bool {
//!         false
//!     }
//! }
//! impl StreamArgs for BaseArgs {
//!     fn window(&self) -> Option<u64> {
//!         Some(self.window)
//!     }
//! }
//! impl StreamArgs for ResetArgs {
//!     fn resets(&self) -> bool {
//!         true
//!     }
//! }
//! /// The same of the whole list.
//! trait StreamArgsList {
//!     fn window(&self) -> Option<u64>;
//!     fn resets(&self) -> bool;
//! }
//! impl StreamArgsList for HNil {
//!     fn window(&self) -> Option<u64> {
//!         None
//!     }
//!     fn resets(&self) -> bool {
//!         false
//!     }
//! }
//! impl<A: StreamArgs, T: StreamArgsList> StreamArgsList for HCons<A, T> {
//!     fn window(&self) -> Option<u64> {
//!         self.head.window().or(self.tail.window())
//!     }
//!     fn resets(&self) -> bool {
//!         self.head.resets() || self.tail.resets()
//!     }
//! }
//!
//! /// A component generic over the list of parts: it resets its streams
//! /// only when the list has the part that does.
//! struct Streams {
//!     window: u64,
//!     resets: bool,
//! }
//! impl<O, L, K: NodeKind> Component<O, L, K> for Streams
//! where
//!     L: PartList<L, Args: StreamArgsList>,
//! {
//!     fn build<P: Pos>(_: &mut K::Start, args: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
//!         Streams { window: args.window().unwrap_or(0), resets: args.resets() }
//!     }
//! }
//!
//! type With = HList![Base, Reset];
//! let s = <Streams as Component<Base, With, Root>>::build::<EmptyPos>(
//!     &mut (),
//!     &hlist![BaseArgs { window: 8 }, ResetArgs],
//!     HNil,
//! );
//! assert_eq!((s.window, s.resets), (8, true));
//! type Without = HList![Base];
//! let s = <Streams as Component<Base, Without, Root>>::build::<EmptyPos>(
//!     &mut (),
//!     &hlist![BaseArgs { window: 8 }],
//!     HNil,
//! );
//! assert_eq!((s.window, s.resets), (8, false));
//! ```
//!
//! A host's bare type is not a component of another crate's part:
//!
//! ```compile_fail,E0210
//! use nitori_driver::{
//!     component::{ArgsOf, Component, PartList},
//!     frunk_core::hlist::HNil,
//!     need::{Pos, View},
//!     structure::NodeKind,
//! };
//! trait Estimator {
//!     fn new() -> Self;
//! }
//! struct Base;
//! impl<X: Estimator + 'static, L: PartList<L>, K: NodeKind> Component<Base, L, K> for X {
//!     fn build<P: Pos>(_: &mut K::Start, _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
//!         X::new()
//!     }
//! }
//! ```
//!
//! it is kept in [`Held`]:
//!
//! ```
//! use nitori_driver::{
//!     component::{ArgsOf, Component, Held, PartList},
//!     frunk_core::hlist::HNil,
//!     id::Numbered,
//!     need::{Pos, View},
//!     structure::NodeKind,
//! };
//! trait Estimator {
//!     fn new() -> Self;
//! }
//! struct Base;
//! impl<X, L, K> Component<Base, L, K> for Held<X>
//! where
//!     X: Estimator + Numbered + 'static,
//!     L: PartList<L>,
//!     K: NodeKind,
//! {
//!     fn build<P: Pos>(_: &mut K::Start, _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
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
    id::Numbered,
    need::{Need, Pos, View},
    structure::NodeKind,
};

/// The construction arguments of the part `Self` in a composition of the
/// parts `L`; their type may depend on the whole list.
pub trait Part<L> {
    /// The arguments.
    type Args;
    /// The shares of this part in the kinds of nodes it extends, one
    /// [`Share`](crate::structure::Share) per kind; none by default. On a
    /// kind it does not list, the part keeps nothing: no components, no
    /// calls and no reactions. A share at a kind the structure it is
    /// composed in does not have is left out, not refused.
    type Shares = HNil;
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
pub trait Component<O, L: PartList<L>, K: NodeKind>: Sized + 'static {
    /// What [`wake`](Component::wake) and [`end`](Component::end) take
    /// around the node besides the component: needs, lent at the node;
    /// none by default.
    type Needs: Need = HNil;
    /// What [`build`](Component::build) takes: needs, lent at the new node
    /// from the nodes above it; none by default. The root's components
    /// have none.
    type BuildNeeds: Need = HNil;
    /// The component of a new node, from what the one adding the node gave
    /// it to start from, the arguments of every part and the view of its
    /// build needs. The node's components are built in the order of the
    /// parts, each handed the same `start`, and may take out what they
    /// keep of it.
    fn build<P: Pos>(
        start: &mut K::Start,
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

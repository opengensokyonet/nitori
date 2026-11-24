//! Reaction points: an action that the other parts react to, where it
//! happens.
//!
//! The part that defines an action defines its reaction point
//! ([`ReactionPoint`]) and what it carries ([`ReactionPoint::Payload`]).
//! A part that reacts lists, in its share of a kind of node, which
//! reaction it has to which point ([`Share::Reacts`](crate::structure::Share::Reacts),
//! a list of [`ReactsTo`]); a reaction ([`Reaction`]) is needs and what
//! it does with their view and the payload. Points are told apart by
//! their numbers, so a part that does not list a point does not react to
//! it, and parts of crates that do not know each other compose.
//!
//! The one that acts takes the point as a capability, [`Cap<P>`](crate::need::Cap),
//! and fires it ([`LentReactions::fire`]). Lent at a node, it takes the
//! reactions listed in the shares of that node's kind and of every kind
//! above it, of every part: the parts in their order, and within a part
//! the kinds from the root down. Their needs are taken around the lend
//! point as the reactions' own, all of them together as the branches of a
//! `+` ([`Plus`](crate::need::Plus)): one reaction may take what another
//! does, and each runs in turn. To react after it has done its own part,
//! the one that acts puts the point in a branch of a `+` of its own, and
//! a reaction may then take what it takes beside it. A reaction may fire
//! another point; the needs are proved through, and a chain that comes
//! back to a point it started from does not compile.
//!
//! The driver fires two points of its own: [`NodeStarted<K>`] at a node
//! of the kind `K` once it is added, and [`NodeEnding<K>`] at it before
//! it ends, which takes only the reactions of the kinds above it.
//!
//! The examples use the structure of [`need`](crate::need), whose part
//! reacts at a link path to [`Sent`](crate::__private::doc::Sent) by
//! counting it in the link's core:
//!
//! ```
//! # use nitori_driver::needs;
//! # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
//! lends!(LinkPath, HList![Ref<Planner>, Cap<Sent>]);
//! lends!(LinkPath, needs![Ref<Planner>, (Mut<LinkCore> + Cap<Sent>)]);
//! ```
//!
//! A reaction that takes what the one that acts takes beside the point
//! overlaps it:
//!
//! ```compile_fail,E0277
//! # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
//! lends!(LinkPath, HList![Ref<LinkCore>, Cap<Sent>]);
//! ```
//!
//! A reaction to [`Echo`](crate::__private::doc::Echo) fires it again:
//!
//! ```compile_fail,E0277
//! # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
//! lends!(LinkPath, HList![Cap<Echo>]);
//! ```
//!
//! A reaction to [`Tick`](crate::__private::doc::Tick) fires
//! [`Tock`](crate::__private::doc::Tock), whose reaction fires `Tick`:
//!
//! ```compile_fail,E0277
//! # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
//! lends!(Conn, HList![Cap<Tick>]);
//! ```
//!
//! The driver's own points are not capabilities:
//!
//! ```compile_fail,E0277
//! # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
//! # use nitori_driver::react::NodeEnding;
//! lends!(Conn, HList![Cap<NodeEnding<LinkPath>>]);
//! ```
//!
//! ```compile_fail,E0277
//! # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
//! # use nitori_driver::react::NodeStarted;
//! lends!(LinkPath, HList![Cap<NodeStarted<LinkPath>>]);
//! ```
use core::marker::PhantomData;

use crate::{
    capability::Capability,
    id::Numbered,
    need::{Need, Pos, View},
    structure::NodeKind,
};

/// A point an action has, that other parts react to: its type-level
/// number tells it apart, and the payload is what the action tells them.
///
/// A part defines its points with it; they are capabilities, taken with
/// [`Cap`](crate::need::Cap) by the one that acts. The driver's own
/// points, [`NodeStarted<K>`] and [`NodeEnding<K>`], are not: only the
/// driver fires them.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a reaction point",
    note = "a part defines a reaction point by implementing `ReactionPoint`; taken with `Cap`, a type is a capability when it implements `Capability` or `ReactionPoint`, and the driver's own points `NodeStarted` and `NodeEnding` are fired by the driver only"
)]
pub trait ReactionPoint: Numbered + 'static {
    /// What a firing carries to every reaction, borrowed for `'a`.
    type Payload<'a>: Copy;
}

/// A point reactions can be listed for: every [`ReactionPoint`], and the
/// driver's own two. Not written by a part: what [`Reaction`] and the
/// lending of reactions need of a point.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a reaction point",
    note = "a part defines a reaction point by implementing `ReactionPoint`"
)]
pub trait Point: sealed::Sealed + Numbered + 'static {
    /// What a firing carries to every reaction, borrowed for `'a`.
    type Payload<'a>: Copy;
}
impl<R: ReactionPoint> Point for R {
    type Payload<'a> = R::Payload<'a>;
}
mod sealed {
    /// Only this crate says what a [`Point`](super::Point) is.
    pub trait Sealed {}
    impl<R: super::ReactionPoint> Sealed for R {}
    impl<K> Sealed for super::NodeStarted<K> {}
    impl<K> Sealed for super::NodeEnding<K> {}
}

/// A part's reaction to the point `P`: its needs, taken around the lend
/// point the point is fired at, and what it does.
pub trait Reaction<P: Point>: 'static {
    /// The needs.
    type Needs: Need;
    /// The point was fired with `payload`; the view of the needs is at the
    /// position `Q`.
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, payload: P::Payload<'_>);
}

/// An item of [`Share::Reacts`](crate::structure::Share::Reacts): the
/// reaction `R` to the point `P`.
pub struct ReactsTo<P, R>(PhantomData<fn() -> (P, R)>);

/// What [`Cap<P>`](crate::need::Cap) of a reaction point `R` is lent as
/// for `'t`, at the position `P`: what every reaction takes, fired in
/// turn.
pub struct LentReactions<'t, R: Point, P: Pos> {
    held: P::Held<'t>,
    run: Run<'t, R, P>,
}
/// How [`LentReactions`] runs its reactions; the last argument binds the
/// payload's lifetime, which a projection alone does not.
type Run<'t, R, P> =
    for<'b, 'p> fn(&'b mut <P as Pos>::Held<'t>, <R as Point>::Payload<'p>, PhantomData<&'p ()>);
impl<'t, R: Point, P: Pos> LentReactions<'t, R, P> {
    /// The reactions held by `held`, run by `run`.
    pub(crate) fn new(held: P::Held<'t>, run: Run<'t, R, P>) -> Self {
        Self { held, run }
    }
    /// Run every reaction with `payload`, one after another, in the order
    /// of the parts and, within a part, of the kinds from the root down.
    pub fn fire(&mut self, payload: R::Payload<'_>) {
        (self.run)(&mut self.held, payload, PhantomData);
    }
}

/// The reactions to `R` around a lend point, from the kinds `W` picks.
/// Not a word of the needs: what [`Cap`](crate::need::Cap) of a reaction
/// point takes.
#[doc(hidden)]
pub struct Reactions<R, W>(PhantomData<fn() -> (R, W)>);
impl<R: Point, W: 'static> Need for Reactions<R, W> {
    type View<'t, P: Pos> = LentReactions<'t, R, P>;
}
/// The kinds of the lend point and every kind above it.
#[doc(hidden)]
pub struct WithLendPoint;
/// Every kind above the lend point, not its own.
#[doc(hidden)]
pub struct AboveLendPoint;

/// A reaction point is the capability of firing it; the driver's own
/// points are not.
impl<R: ReactionPoint> Capability for R {
    type Needs = Reactions<R, WithLendPoint>;
    type Object<'t, P: Pos> = LentReactions<'t, R, P>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Object<'t, P> {
        view
    }
}

/// A node of the kind `K` was added: the driver fires it at the node, with
/// the reactions of `K` and of the kinds above it. Only the driver fires
/// it: it is not a capability.
#[derive(Numbered)]
pub struct NodeStarted<K>(PhantomData<fn() -> K>);
impl<K: NodeKind> Point for NodeStarted<K>
where
    Self: Numbered,
{
    type Payload<'a> = ();
}

/// A node of the kind `K` is to end: the driver fires it at the node,
/// still in the tree, with the reactions of the kinds above `K` only.
/// Only the driver fires it: it is not a capability.
#[derive(Numbered)]
pub struct NodeEnding<K>(PhantomData<fn() -> K>);
impl<K: NodeKind> Point for NodeEnding<K>
where
    Self: Numbered,
{
    type Payload<'a> = ();
}

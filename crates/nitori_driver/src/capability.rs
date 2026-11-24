//! Capabilities: what a call takes besides components, put together from
//! needs.
//!
//! A composed capability ([`Capability`], taken with
//! [`Cap`](crate::need::Cap)) is a list of needs and a way to put their
//! view together into an object. It declares no node to start from: its
//! needs are taken around the lend point of the need that takes it, proved
//! with the other needs as if they had been written there, so an overlap
//! between the two does not compile. Taken elsewhere, it is in an
//! [`At`](crate::need::At). Every [`ReactionPoint`](crate::react::ReactionPoint)
//! is a capability too, whose object fires the reactions.
//!
//! A contributed capability (taken with [`Contributed<F>`](Contributed)) is
//! made of every part's piece for the family `F`. Each part writes
//! [`Contribute<F>`] for every family a call of the composition uses,
//! `Needs = HNil` and `Piece<'t> = ()` when it gives nothing. The pieces
//! are lined up in the order of the parts, as an `HList` ([`PiecesOf`]);
//! the family writes its rule on that list once. Two rules are given here:
//! dispatching a sum to the piece of its alternative ([`DispatchSum`],
//! each piece writing [`DispatchRun`]), and running every piece in turn
//! ([`EachPiece`], each piece writing [`EachRun`]). A piece of `()`
//! dispatches the empty sum and runs as nothing, written once.
//!
//! The examples use the structure of [`need`](crate::need). Taking a
//! link's planning, with the connection's core beside it:
//!
//! ```
//! # use nitori_driver::{__private::doc::*, frunk_core::{HList, hlist_pat}, need::*};
//! use nitori_driver::capability::Capability;
//!
//! /// Planning a packet: the planner, exclusive, and the link's core.
//! struct Planning;
//! struct Plan<'t> {
//!     planner: &'t mut Planner,
//!     core: &'t LinkCore,
//! }
//! impl Capability for Planning {
//!     type Needs = HList![Mut<Planner>, Ref<LinkCore>];
//!     type Object<'t, P: Pos> = Plan<'t>;
//!     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Plan<'t> {
//!         let hlist_pat![planner, core] = view;
//!         Plan { planner, core }
//!     }
//! }
//! lends!(LinkPath, HList![Cap<Planning>, Ref<ConnCore>]);
//! // Every link's planning, from the connection.
//! lends!(Conn, HList![Under<ConnCore, At<Planner, Cap<Planning>>>]);
//! ```
//!
//! A contributed capability, the parts' labels read in turn; the one part
//! of the structure contributes its label:
//!
//! ```
//! # use nitori_driver::{__private::doc::*, frunk_core::{HList, hlist_pat}, need::*};
//! use nitori_driver::capability::{Contribute, Contributed, EachRun};
//!
//! /// The family: every part's label, counted.
//! struct Labels;
//! struct LabelPiece<'t>(&'t Label);
//! impl EachRun<Labels, u32> for LabelPiece<'_> {
//!     fn each_run(&mut self, count: &mut u32) {
//!         *count += 1;
//!     }
//! }
//! impl Contribute<Labels> for V1 {
//!     type Needs = HList![Ref<Label>];
//!     type Piece<'t> = LabelPiece<'t>;
//!     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> LabelPiece<'t> {
//!         let hlist_pat![label] = view;
//!         LabelPiece(label)
//!     }
//! }
//! lends!(Conn, HList![Contributed<Labels>, Ref<ConnCore>]);
//! ```
//!
//! A part's piece and another need taking the same component, one of them
//! exclusive, overlap:
//!
//! ```compile_fail,E0277
//! # use nitori_driver::{__private::doc::*, frunk_core::{HList, hlist_pat}, need::*};
//! # use nitori_driver::capability::{Contribute, Contributed};
//! # struct Labels;
//! # impl Contribute<Labels> for V1 {
//! #     type Needs = HList![Ref<Label>];
//! #     type Piece<'t> = &'t Label;
//! #     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> &'t Label {
//! #         let hlist_pat![label] = view;
//! #         label
//! #     }
//! # }
//! lends!(Conn, HList![Contributed<Labels>, Mut<Label>]);
//! ```
use core::marker::PhantomData;

use frunk_core::{
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil},
};

use crate::need::{Need, Pos, View};

// ---------------------------------------------------------------- composed capabilities

/// A composed capability: needs, and the object made of their view.
///
/// The needs are taken around the lend point of the
/// [`Cap`](crate::need::Cap) that takes it; they may take other
/// capabilities, and `+`. The object depends on the position `P` of the
/// needs only as their view does, so code generic over the structure uses
/// it through what it declares.
pub trait Capability: 'static {
    /// The needs.
    type Needs: Need;
    /// The object, lent for `'t`, of the needs' view at `P`.
    type Object<'t, P: Pos>;
    /// Put the object together.
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Object<'t, P>;
}

// ---------------------------------------------------------------- contributed capabilities

/// Every part's piece for the family `F`, in the order of the parts: an
/// `HList` of them ([`PiecesOf`]).
///
/// What it is lent as follows from the parts of the composition, not from
/// the need: where they are known, the list of the pieces.
pub struct Contributed<F>(PhantomData<fn() -> F>);
impl<F: 'static> Need for Contributed<F> {
    type View<'t, P: Pos> = P::Held<'t>;
}

/// What the part `Self` contributes to the family `F`: needs, and the
/// piece put together from their view.
///
/// Every part of a composition writes it for every family a call of the
/// composition uses; a part that gives nothing writes `Needs = HNil` and
/// `Piece<'t> = ()`. The needs are taken around the lend point, as a
/// composed capability's are, and proved with the other needs; the piece
/// does not depend on their position.
#[diagnostic::on_unimplemented(
    message = "the part `{Self}` does not say what it contributes to the family `{F}`",
    note = "every part writes `Contribute<{F}>` when a need of the composition takes `Contributed<{F}>`; `Needs = HNil` and `Piece<'t> = ()` for nothing"
)]
pub trait Contribute<F> {
    /// The needs.
    type Needs: Need;
    /// The piece, lent for `'t`.
    type Piece<'t>;
    /// Put the piece together.
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Piece<'t>;
}

/// The pieces of a list of parts for the family `F`, in the order of the
/// parts.
pub trait PieceList<F>: 'static {
    /// Every part's needs, one list per part.
    type Needs: Need;
    /// Every part's piece.
    type Pieces<'t>;
    /// Put every piece together from the view of every part's needs.
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Pieces<'t>;
}
impl<F> PieceList<F> for HNil {
    type Needs = HNil;
    type Pieces<'t> = HNil;
    fn assemble<'t, P: Pos>(_: View<'t, Self::Needs, P>) -> Self::Pieces<'t> {
        HNil
    }
}
impl<F, Pt: Contribute<F> + 'static, T: PieceList<F>> PieceList<F> for HCons<Pt, T> {
    type Needs = HCons<Pt::Needs, T::Needs>;
    type Pieces<'t> = HCons<Pt::Piece<'t>, T::Pieces<'t>>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Pieces<'t> {
        HCons {
            head: Pt::assemble::<P::Head>(view.head),
            tail: T::assemble::<P::Tail>(view.tail),
        }
    }
}
/// The pieces of the parts `P` for the family `F`, lent for `'t`.
pub type PiecesOf<'t, P, F> = <P as PieceList<F>>::Pieces<'t>;

// ---------------------------------------------------------------- the two rules

/// A piece of the family `F` dispatches `R`, its part's alternative of a
/// sum, with the argument `A`, giving `O`.
///
/// The piece does not name its part's type for `R`; the sum is defined
/// once elsewhere (for frames, by the wire format), and [`DispatchSum`]
/// lines it up with the pieces by position.
pub trait DispatchRun<F, R, A, O> {
    /// Dispatch the item.
    fn dispatch_run(&mut self, item: R, arg: A) -> O;
}
/// The piece of a part that gives nothing dispatches the empty sum.
impl<F, A, O> DispatchRun<F, CNil, A, O> for () {
    fn dispatch_run(&mut self, item: CNil, _: A) -> O {
        match item {}
    }
}
/// The pieces of the family `F` dispatch the sum `R`: the head of the sum
/// to the first piece, the rest to the others.
pub trait DispatchSum<F, R, A, O> {
    /// Dispatch the item to the piece of its alternative.
    fn dispatch_sum(&mut self, item: R, arg: A) -> O;
}
impl<F, A, O> DispatchSum<F, CNil, A, O> for HNil {
    fn dispatch_sum(&mut self, item: CNil, _: A) -> O {
        match item {}
    }
}
impl<F, A, O, H, T, RH, RT> DispatchSum<F, Coproduct<RH, RT>, A, O> for HCons<H, T>
where
    H: DispatchRun<F, RH, A, O>,
    T: DispatchSum<F, RT, A, O>,
{
    fn dispatch_sum(&mut self, item: Coproduct<RH, RT>, arg: A) -> O {
        match item {
            Coproduct::Inl(h) => self.head.dispatch_run(h, arg),
            Coproduct::Inr(t) => self.tail.dispatch_sum(t, arg),
        }
    }
}

/// A piece of the family `F` run in turn with the argument `A`.
pub trait EachRun<F, A> {
    /// Run the piece.
    fn each_run(&mut self, arg: &mut A);
}
/// The piece of a part that gives nothing runs as nothing.
impl<F, A> EachRun<F, A> for () {
    fn each_run(&mut self, _: &mut A) {}
}
/// The pieces of the family `F` run every piece in turn, in the order of
/// the parts.
pub trait EachPiece<F, A> {
    /// Run every piece.
    fn each_piece(&mut self, arg: &mut A);
}
impl<F, A> EachPiece<F, A> for HNil {
    fn each_piece(&mut self, _: &mut A) {}
}
impl<F, A, H: EachRun<F, A>, T: EachPiece<F, A>> EachPiece<F, A> for HCons<H, T> {
    fn each_piece(&mut self, arg: &mut A) {
        self.head.each_run(arg);
        self.tail.each_piece(arg);
    }
}

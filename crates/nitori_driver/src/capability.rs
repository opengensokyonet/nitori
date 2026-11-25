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
//! A contributed capability (taken with [`Contributed<F, Q>`](Contributed)) is
//! made of every part's piece for the family `F`, the parts being those
//! of the composition `Q`. A part's contribution to a family is a type
//! implementing [`Contribute<F>`]: its needs, and the piece put together
//! from their view. It is written in one of two places: in the part's own
//! list, [`Part::Contributions`](crate::component::Part::Contributions),
//! as a [`Contribution<F, C>`](Contribution); or in the composition's,
//! [`Composition::Contributions`](crate::call::Composition::Contributions),
//! as a [`PartContribution<O, F, C>`](PartContribution) of the part `O`,
//! for a part that does not know the family. The crate of a family may
//! export a list of these for the parts it knows, which the composition
//! puts in its own list as one item, spliced in. The part and the family
//! are found by their numbers: a family is `#[derive(Numbered)]`, and,
//! when the composition's list is not empty, so is every part of the
//! composition, not only those the list names (as every part is when a
//! component finds another's arguments with
//! [`part_args`](crate::component::part_args)). A part that writes nothing
//! for the family, in either place, contributes `()`, with no needs. A
//! contribution written in both places, or twice in one list (the lists
//! spliced in counted), does not compile where the composition is written,
//! whether or not a need takes the family, and neither does a
//! composition's list that names a part the composition does not have.
//!
//! The pieces are lined up in the order of the parts, as an `HList`
//! ([`PiecesOf`]); the family writes its rule on that list once. Two rules
//! are given here: dispatching a sum to the piece of its alternative
//! ([`DispatchSum`], each piece writing [`DispatchRun`]), and running
//! every piece in turn ([`EachPiece`], each piece writing [`EachRun`]). A
//! piece of `()` dispatches the empty sum and runs as nothing, written
//! once.
//!
//! A family, a part that lists its contribution, a part whose contribution
//! the composition's list gives, spliced in from what the family's crate
//! exports, and a part with none:
//!
//! ```
//! #![feature(impl_trait_in_assoc_type)]
//! use nitori_driver::{
//!     __private::calls::{Count, Item, Root, StillClock, Total},
//!     capability::{Contribute, Contribution, EachPiece, EachRun, PartContribution, PiecesOf},
//!     component::Part,
//!     composition,
//!     frunk_core::{HList, hlist_pat},
//!     id::Numbered,
//!     need::{Pos, Ref, View},
//!     structure::Share,
//! };
//!
//! /// The family: what every part says of the total, counted.
//! #[derive(Numbered)]
//! struct Tally;
//! struct TotalPiece<'t>(&'t Total);
//! impl EachRun<Tally, u32> for TotalPiece<'_> {
//!     fn each_run(&mut self, n: &mut u32) {
//!         *n += self.0.0 + 1;
//!     }
//! }
//! /// A contribution to the family: the root's total, read.
//! struct TotalTally;
//! impl Contribute<Tally> for TotalTally {
//!     type Needs = HList![Ref<Total>];
//!     type Piece<'t> = TotalPiece<'t>;
//!     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> TotalPiece<'t> {
//!         let hlist_pat![total] = view;
//!         TotalPiece(total)
//!     }
//! }
//!
//! /// A part that knows the family, and lists its contribution.
//! #[derive(Numbered)]
//! struct Counter;
//! impl<L> Part<L> for Counter {
//!     type Args = ();
//!     type Shares = HList![Share<Root, HList![Total]>, Share<Item, HList![Count]>];
//!     type Contributions = HList![Contribution<Tally, TotalTally>];
//! }
//! /// Two parts that do not know it.
//! #[derive(Numbered)]
//! struct Audit;
//! impl<L> Part<L> for Audit {
//!     type Args = ();
//! }
//! #[derive(Numbered)]
//! struct Quiet;
//! impl<L> Part<L> for Quiet {
//!     type Args = ();
//! }
//! /// What the family's crate exports for the parts it knows.
//! type TallyOfAudit = HList![PartContribution<Audit, Tally, TotalTally>];
//!
//! struct Q;
//! composition!(impl for Q: Root, HList![Counter, Audit, Quiet], StillClock, u8, HList![TallyOfAudit]);
//!
//! // The pieces, in the order of the parts; the quiet part's is `()`.
//! fn lined_up<'t>(pieces: PiecesOf<'t, Q, Tally>) -> HList![TotalPiece<'t>, TotalPiece<'t>, ()] {
//!     pieces
//! }
//! fn count(mut pieces: PiecesOf<'_, Q, Tally>) -> u32 {
//!     let mut n = 0;
//!     pieces.each_piece(&mut n);
//!     n
//! }
//! ```
//!
//! The counter's contribution written in the composition's list as well,
//! though no need takes the family:
//!
//! ```compile_fail,E0277
//! # #![feature(impl_trait_in_assoc_type)]
//! # use nitori_driver::{
//! #     __private::calls::{Count, Item, Root, StillClock, Total},
//! #     capability::{Contribute, Contribution, PartContribution},
//! #     component::Part,
//! #     composition,
//! #     frunk_core::{HList, hlist_pat},
//! #     id::Numbered,
//! #     need::{Pos, Ref, View},
//! #     structure::Share,
//! # };
//! # #[derive(Numbered)]
//! # struct Tally;
//! # struct TotalTally;
//! # impl Contribute<Tally> for TotalTally {
//! #     type Needs = HList![Ref<Total>];
//! #     type Piece<'t> = &'t Total;
//! #     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> &'t Total {
//! #         let hlist_pat![total] = view;
//! #         total
//! #     }
//! # }
//! # #[derive(Numbered)]
//! # struct Counter;
//! # impl<L> Part<L> for Counter {
//! #     type Args = ();
//! #     type Shares = HList![Share<Root, HList![Total]>, Share<Item, HList![Count]>];
//! #     type Contributions = HList![Contribution<Tally, TotalTally>];
//! # }
//! struct Q;
//! composition!(
//!     impl for Q: Root, HList![Counter], StillClock, u8,
//!     HList![PartContribution<Counter, Tally, TotalTally>],
//! );
//! ```
//!
//! Written twice in the composition's list:
//!
//! ```compile_fail,E0277
//! # #![feature(impl_trait_in_assoc_type)]
//! # use nitori_driver::{
//! #     __private::calls::{Count, Item, Root, StillClock, Total},
//! #     capability::{Contribute, Contribution, PartContribution},
//! #     component::Part,
//! #     composition,
//! #     frunk_core::{HList, hlist_pat},
//! #     id::Numbered,
//! #     need::{Pos, Ref, View},
//! #     structure::Share,
//! # };
//! # #[derive(Numbered)]
//! # struct Tally;
//! # struct TotalTally;
//! # impl Contribute<Tally> for TotalTally {
//! #     type Needs = HList![Ref<Total>];
//! #     type Piece<'t> = &'t Total;
//! #     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> &'t Total {
//! #         let hlist_pat![total] = view;
//! #         total
//! #     }
//! # }
//! # #[derive(Numbered)]
//! # struct Counter;
//! # impl<L> Part<L> for Counter {
//! #     type Args = ();
//! #     type Shares = HList![Share<Root, HList![Total]>, Share<Item, HList![Count]>];
//! # }
//! struct Q;
//! composition!(
//!     impl for Q: Root, HList![Counter], StillClock, u8,
//!     HList![
//!         PartContribution<Counter, Tally, TotalTally>,
//!         PartContribution<Counter, Tally, TotalTally>,
//!     ],
//! );
//! ```
//!
//! Written twice in the part's own list:
//!
//! ```compile_fail,E0277
//! # #![feature(impl_trait_in_assoc_type)]
//! # use nitori_driver::{
//! #     __private::calls::{Count, Item, Root, StillClock, Total},
//! #     capability::{Contribute, Contribution, PartContribution},
//! #     component::Part,
//! #     composition,
//! #     frunk_core::{HList, hlist_pat},
//! #     id::Numbered,
//! #     need::{Pos, Ref, View},
//! #     structure::Share,
//! # };
//! # #[derive(Numbered)]
//! # struct Tally;
//! # struct TotalTally;
//! # impl Contribute<Tally> for TotalTally {
//! #     type Needs = HList![Ref<Total>];
//! #     type Piece<'t> = &'t Total;
//! #     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> &'t Total {
//! #         let hlist_pat![total] = view;
//! #         total
//! #     }
//! # }
//! # #[derive(Numbered)]
//! # struct Counter;
//! # impl<L> Part<L> for Counter {
//! #     type Args = ();
//! #     type Shares = HList![Share<Root, HList![Total]>, Share<Item, HList![Count]>];
//! #     type Contributions = HList![Contribution<Tally, TotalTally>, Contribution<Tally, TotalTally>];
//! # }
//! struct Q;
//! composition!(impl for Q: Root, HList![Counter], StillClock, u8);
//! ```
//!
//! Written in a list spliced in and again beside it:
//!
//! ```compile_fail,E0277
//! # #![feature(impl_trait_in_assoc_type)]
//! # use nitori_driver::{
//! #     __private::calls::{Count, Item, Root, StillClock, Total},
//! #     capability::{Contribute, Contribution, PartContribution},
//! #     component::Part,
//! #     composition,
//! #     frunk_core::{HList, hlist_pat},
//! #     id::Numbered,
//! #     need::{Pos, Ref, View},
//! #     structure::Share,
//! # };
//! # #[derive(Numbered)]
//! # struct Tally;
//! # struct TotalTally;
//! # impl Contribute<Tally> for TotalTally {
//! #     type Needs = HList![Ref<Total>];
//! #     type Piece<'t> = &'t Total;
//! #     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> &'t Total {
//! #         let hlist_pat![total] = view;
//! #         total
//! #     }
//! # }
//! # #[derive(Numbered)]
//! # struct Counter;
//! # impl<L> Part<L> for Counter {
//! #     type Args = ();
//! #     type Shares = HList![Share<Root, HList![Total]>, Share<Item, HList![Count]>];
//! # }
//! type TallyOfCounter = HList![PartContribution<Counter, Tally, TotalTally>];
//! struct Q;
//! composition!(
//!     impl for Q: Root, HList![Counter], StillClock, u8,
//!     HList![TallyOfCounter, PartContribution<Counter, Tally, TotalTally>],
//! );
//! ```
//!
//! A composition's list that names a part the composition does not have:
//!
//! ```compile_fail,E0277
//! # #![feature(impl_trait_in_assoc_type)]
//! # use nitori_driver::{
//! #     __private::calls::{Count, Item, Root, StillClock, Total},
//! #     capability::{Contribute, PartContribution},
//! #     component::Part,
//! #     composition,
//! #     frunk_core::{HList, hlist_pat},
//! #     id::Numbered,
//! #     need::{Pos, Ref, View},
//! #     structure::Share,
//! # };
//! # #[derive(Numbered)]
//! # struct Tally;
//! # struct TotalTally;
//! # impl Contribute<Tally> for TotalTally {
//! #     type Needs = HList![Ref<Total>];
//! #     type Piece<'t> = &'t Total;
//! #     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> &'t Total {
//! #         let hlist_pat![total] = view;
//! #         total
//! #     }
//! # }
//! # #[derive(Numbered)]
//! # struct Counter;
//! # impl<L> Part<L> for Counter {
//! #     type Args = ();
//! #     type Shares = HList![Share<Root, HList![Total]>, Share<Item, HList![Count]>];
//! # }
//! #[derive(Numbered)]
//! struct Audit;
//! struct Q;
//! composition!(
//!     impl for Q: Root, HList![Counter], StillClock, u8,
//!     HList![PartContribution<Audit, Tally, TotalTally>],
//! );
//! ```
//!
//! The examples that follow use the structure of [`need`](crate::need). Taking a
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
//! A contributed capability lent, the parts' labels read in turn; the one
//! part of the structure contributes its label, which the composition
//! `Q` lists:
//!
//! ```
//! # use nitori_driver::{__private::doc::*, frunk_core::{HList, hlist_pat}, id::Numbered, need::*};
//! use nitori_driver::capability::{Contribute, Contributed, EachPiece, EachRun, PartContribution, PiecesOf};
//!
//! /// The family: every part's label, counted.
//! #[derive(Numbered)]
//! struct Labels;
//! struct LabelPiece<'t>(&'t Label);
//! impl EachRun<Labels, u32> for LabelPiece<'_> {
//!     fn each_run(&mut self, count: &mut u32) {
//!         *count += 1;
//!     }
//! }
//! /// What `V1` contributes: its label.
//! struct V1Labels;
//! impl Contribute<Labels> for V1Labels {
//!     type Needs = HList![Ref<Label>];
//!     type Piece<'t> = LabelPiece<'t>;
//!     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> LabelPiece<'t> {
//!         let hlist_pat![label] = view;
//!         LabelPiece(label)
//!     }
//! }
//! # type Q = Composed<HList![PartContribution<V1, Labels, V1Labels>]>;
//! lends!(Conn, HList![Contributed<Labels, Q>, Ref<ConnCore>]);
//!
//! // The view is the list of the pieces, whatever the position.
//! fn count(mut pieces: PiecesOf<'_, Q, Labels>) -> u32 {
//!     let mut n = 0;
//!     pieces.each_piece(&mut n);
//!     n
//! }
//! ```
//!
//! A part's piece and another need taking the same component, one of them
//! exclusive, overlap:
//!
//! ```compile_fail,E0277
//! # use nitori_driver::{__private::doc::*, frunk_core::{HList, hlist_pat}, id::Numbered, need::*};
//! # use nitori_driver::capability::{Contribute, Contributed, PartContribution};
//! # #[derive(Numbered)]
//! # struct Labels;
//! # struct V1Labels;
//! # impl Contribute<Labels> for V1Labels {
//! #     type Needs = HList![Ref<Label>];
//! #     type Piece<'t> = &'t Label;
//! #     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> &'t Label {
//! #         let hlist_pat![label] = view;
//! #         label
//! #     }
//! # }
//! # type Q = Composed<HList![PartContribution<V1, Labels, V1Labels>]>;
//! lends!(Conn, HList![Contributed<Labels, Q>, Mut<Label>]);
//! ```
use core::marker::PhantomData;

use frunk_core::{
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil},
};

use crate::need::{Need, Pos, View};

pub(crate) mod contribution;
pub use contribution::{Contribution, PartContribution, PieceList, PiecesOf};

// ---------------------------------------------------------------- composed capabilities

/// A composed capability: needs, and the object made of their view.
///
/// The needs are taken around the lend point of the
/// [`Cap`](crate::need::Cap) that takes it; they may take other
/// capabilities, and `+`. The object depends on the position `P` of the
/// needs only as their view does, so code generic over the structure uses
/// it through what it declares.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a capability",
    note = "a capability is a composed one, implementing `Capability`, or a part's reaction point, implementing `ReactionPoint`; the driver's own points `NodeStarted` and `NodeEnding` are fired by the driver only"
)]
pub trait Capability: 'static {
    /// The needs.
    type Needs: Need;
    /// The object, lent for `'t`, of the needs' view at `P`.
    type Object<'t, P: Pos>;
    /// Put the object together.
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Object<'t, P>;
}

// ---------------------------------------------------------------- contributed capabilities

/// Every part's piece for the family `F`, the parts being those of the
/// composition `Q`, in their order: an `HList` of them,
/// [`PiecesOf`]`<'t, Q, F>`.
///
/// `Q` is the composition the need is lent under: written out where it is
/// known, the call's own `Q` in a call generic over it, which uses the
/// pieces through what a bound of the composition ([`PieceList`]) says of
/// them. The view does not depend on where it is lent. The pieces' needs
/// are taken around the lend point and proved with the other needs; lent
/// in a structure whose parts are not those of `Q`, it does not compile.
///
/// A call of the composition `Q` taking the pieces of another, `Q2`,
/// whose parts are not `Q`'s:
///
/// ```compile_fail,E0277
/// # #![feature(impl_trait_in_assoc_type)]
/// # use nitori_call::{LentAccess, call};
/// # use nitori_driver::{
/// #     __private::calls::{Root, StillClock, Total},
/// #     call::{CallFamily, CallKind, Composition, LoanNeeds},
/// #     capability::{Contribute, Contributed, Contribution},
/// #     component::Part,
/// #     composition,
/// #     frunk_core::{HList, hlist_pat},
/// #     id::Numbered,
/// #     need::{Pos, Ref, View},
/// #     structure::{NodeKind, Share},
/// # };
/// # #[derive(Numbered)]
/// # struct Tally;
/// # struct TotalTally;
/// # impl Contribute<Tally> for TotalTally {
/// #     type Needs = HList![Ref<Total>];
/// #     type Piece<'t> = &'t Total;
/// #     fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> &'t Total {
/// #         let hlist_pat![total] = view;
/// #         total
/// #     }
/// # }
/// # #[derive(Numbered)]
/// # struct Counter;
/// # impl<L> Part<L> for Counter {
/// #     type Args = ();
/// #     type Shares = HList![Share<Root, HList![Total]>];
/// #     type Contributions = HList![Contribution<Tally, TotalTally>];
/// # }
/// # #[derive(Numbered)]
/// # struct Audit;
/// # impl<L> Part<L> for Audit {
/// #     type Args = ();
/// # }
/// impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Counting {
///     type Needs = HList![Contributed<Tally, Q2>];
///     type Op = Self;
///     fn start(_: &mut K::Start) -> Self {
///         Counting::new()
///     }
/// }
/// #[call]
/// async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
///     host.with(|_| ());
/// }
/// struct Calls;
/// impl<L> Part<L> for Calls {
///     type Args = ();
///     type Shares = HList![Share<Root, HList![], HList![Counting]>];
/// }
/// struct Q2;
/// composition!(impl for Q2: Root, HList![Counter, Audit], StillClock, u8);
/// struct Q;
/// composition!(impl for Q: Root, HList![Counter, Calls], StillClock, u8);
/// ```
pub struct Contributed<F, Q>(PhantomData<fn() -> (F, Q)>);
impl<F: 'static, Q: PieceList<F>> Need for Contributed<F, Q> {
    type View<'t, P: Pos> = Q::Pieces<'t>;
}

/// What the type `Self` contributes to the family `F`: needs, and the
/// piece put together from their view.
///
/// A part's contribution is listed in the part's own list
/// ([`Contribution`]) or in the composition's ([`PartContribution`]); a
/// part with none contributes `()`, whose needs are none and whose piece is
/// `()`. The needs are taken around the lend point, as a composed
/// capability's are, and proved with the other needs; the piece does not
/// depend on their position.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is listed as a contribution to the family `{F}` and does not implement `Contribute<{F}>`",
    note = "a contribution, listed in a part's `Contributions` as `Contribution<{F}, C>` or in the composition's as `PartContribution<O, {F}, C>`, implements `Contribute<{F}>`: its needs, and the piece put together from them"
)]
pub trait Contribute<F> {
    /// The needs.
    type Needs: Need;
    /// The piece, lent for `'t`.
    type Piece<'t>;
    /// Put the piece together.
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Self::Piece<'t>;
}
/// The contribution of a part that has none: no needs, and the piece `()`.
impl<F> Contribute<F> for () {
    type Needs = HNil;
    type Piece<'t> = ();
    fn assemble<'t, P: Pos>(_: View<'t, Self::Needs, P>) -> Self::Piece<'t> {}
}

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

//! Calls lent at nodes: their kinds, their family, and the composition
//! that proves every call of a tree once.
//!
//! A kind of call ([`CallKind`]) is the struct a `#[call]` function of
//! `nitori_call` generates: it gives the call's needs and its operation.
//! The function is generic over its family, `W: CallFamily<C = Kind>`;
//! each poll lends it a new view of its needs, which it reaches with
//! [`LoanNeeds::needs`] inside `host.with(…)`. Between polls a call keeps
//! only its own state.
//!
//! The family is [`Lent<Q, C, KP>`](Lent): the composition `Q`, the kind
//! `C`, and the positions `KP` of the call's keyed needs. Only the lender
//! names it; the call's code sees `KP` through the bound
//! [`CallFamily::KP`] carries, so it names no position.
//!
//! A composition ([`Composition`], written with [`composition!`](crate::composition!)) names a
//! root kind and a list of parts. The calls come from the parts' shares
//! ([`ShareCalls`]), node kind by node kind; every call's positions are
//! inferred once, in the composition's impl, into the opaque
//! [`Composition::Pack`]. [`start`] and [`poll`] reach a call of the pack
//! by its kind.
//!
//! A call on the root that takes every item by key, polled once:
//!
//! ```
//! # #![feature(impl_trait_in_assoc_type, yield_expr)]
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
//! #     composition,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
//! #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
//! /// On the root: every item by key, one poll.
//! impl<Q: Composition> CallKind<Q> for Counting {
//!     type Needs = HList![At<Up<0>, Here, Mut<Count>>, At<Up<0>, ToChild<Item, ByKey<u32>>, Mut<Count>>];
//!     type Op = Self;
//!     fn start() -> Self {
//!         Self::new()
//!     }
//! }
//! #[call]
//! async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
//!     host.with(|mut view| {
//!         let hlist_pat![own, items] = view.needs();
//!         for (_, c) in items.iter_mut() {
//!             own.0 += c.0;
//!         }
//!     });
//! }
//! struct Part;
//! impl Share<Root> for Part {
//!     type Data = HList![Count];
//! }
//! impl Share<Item> for Part {
//!     type Data = HList![Count];
//! }
//! impl ShareCalls<Root> for Part {
//!     type Calls = HList![Counting];
//! }
//! impl ShareCalls<Item> for Part {
//!     type Calls = HNil;
//! }
//! struct Q;
//! composition!(impl for Q: Root, HList![Part]);
//!
//! let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! let _: ItemAt = insert(&mut root, End, 1, Node::new(frunk_core::hlist![frunk_core::hlist![Count(2)]])).ok().unwrap();
//! let mut call = core::pin::pin!(start::<Q, Counting, _, _>(&End));
//! let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
//! let step = poll::<Q, Counting, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), call.as_mut(), &mut cx);
//! assert!(matches!(step, Ok(CallStep::Finished)));
//! assert_eq!(root.data.head.head.0, 2);
//! ```

mod composition;
mod kind;
mod pack;
mod prove;
mod slot;

pub use composition::{CallSlotsOf, Composition, LendInstant, OutboxOf, PartsOf, RootOf};
pub use kind::{
    CallFamily, CallKind, CallStep, KeyedOf, Lent, Loan, LoanNeeds, Positions, RunsOn, View,
};
pub use pack::{
    LendOne, NoCall, NoPos, PackList, PackNth, PositionsAt, RunningAt, SlotAt, SlotCall, poll,
    start,
};
pub use prove::{ChainAt, Prove, ProveOne, SlotPositions, explain, prove};
pub use slot::{
    AtPath, ChildSlots, FindSlot, ListShape, NoSlot, NthItem, PartsCalls, ShareCalls, Slot,
    SlotDesc, SlotYield, SlotsUnder,
};

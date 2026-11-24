//! Calls lent at nodes: their kinds, their family, and the composition
//! that proves every call of a structure once.
//!
//! A kind of call ([`CallKind`]) is the struct a `#[call]` function of
//! `nitori_call` generates: it gives the call's needs, its operation and
//! how a new node's call starts from what the node was added with. The
//! function is generic over its family, `W: CallFamily<C = Kind>`; each
//! poll lends it a new view of its needs, which it reaches with
//! [`LoanNeeds::needs`] inside `host.with(…)`. Between polls a call keeps
//! only its own state; when it no longer needs to run, it returns, and the
//! driver drops it.
//!
//! The family is [`Lent<Q, K, C, KP>`](Lent): the composition `Q`, the
//! node kind `K`, the kind of call `C` and the position `KP` of its
//! needs. Only the lender names it; the call's code names no position,
//! and writes what it asks of the composition as a bound of `Q` in its
//! family bound (`W: CallFamily<C = Receive, Q: DispatchesFrames>`).
//!
//! A composition ([`Composition`], written with
//! [`composition!`](crate::composition!)) names a root kind, a list of
//! parts, a clock and a priority. The calls come from the parts' shares
//! ([`Share::Calls`](crate::structure::Share::Calls)), kind by kind; every
//! call, every component's operations, every kind's build and the
//! driver's reaction points are proved once, where it is written, into
//! the opaque [`Composition::Pack`], through which the driver lends them.
//!
//! A call on the root, written for any structure, that totals every
//! item's count, and the composition it is in:
//!
//! ```
//! #![feature(impl_trait_in_assoc_type)]
//! use nitori_call::{LentAccess, call};
//! use nitori_driver::{
//!     __private::calls::{Count, Item, Root, StillClock, Total},
//!     call::{CallFamily, CallKind, Composition, LoanNeeds},
//!     component::Part,
//!     composition,
//!     frunk_core::{HList, hlist, hlist_pat},
//!     need::{Members, Mut, Ref, Under},
//!     structure::{NodeKind, Share},
//! };
//!
//! /// On the root: the total of every item's count.
//! impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Counting {
//!     type Needs = HList![Mut<Total>, Under<Total, Ref<Count>>];
//!     type Op = Self;
//!     fn start(_: &mut K::Start) -> Self {
//!         Counting::new()
//!     }
//! }
//! #[call]
//! async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
//!     host.with(|mut view| {
//!         let hlist_pat![total, items] = view.needs();
//!         total.0 = items.iter().map(|(_, c)| c.0).sum();
//!     });
//! }
//!
//! struct Counter;
//! impl<L> Part<L> for Counter {
//!     type Args = ();
//! }
//! impl Share<Root> for Counter {
//!     type Data = HList![Total];
//!     type Calls = HList![Counting];
//! }
//! impl Share<Item> for Counter {
//!     type Data = HList![Count];
//! }
//! struct Q;
//! composition!(impl for Q: Root, HList![Counter], StillClock, u8);
//!
//! fn main() {
//!     let root = Q::root(&mut (), &hlist![()]);
//!     assert_eq!(root.data.head.head.0, 0);
//! }
//! ```

mod composition;
mod host;
mod index;
mod kind;
mod kinds;
mod pack;
mod prove;
#[cfg(test)]
pub(crate) mod tests;

pub use composition::{
    ClockOf, Composition, ContextOf, KindsOf, PartsOf, PriorityOf, RootNode, RootOf,
};
#[allow(unused_imports)]
pub(crate) use composition::{
    RequestListsOf, context, request_lists, set_now, swap_wakes, take_requests, take_wakes,
};
#[allow(unused_imports)]
pub(crate) use host::CallHost;
pub use host::Settle;
pub use kind::{CallFamily, CallKind, CallView, Lent, Loan, LoanHost, LoanNeeds, RunsOn};
pub use kinds::OtherKind;
#[allow(unused_imports)]
pub(crate) use pack::{build, end, node_ending, node_started, poll_call, start_calls, wake};
pub use prove::{BuildAt, ProveCall, ProveComponent, ProveHandler, ProveKind};
// The submodules name one another's items through here.
#[allow(unused_imports)]
use self::{
    __pack::*,
    kinds::{CallsOf, CompsOf, KindNotIn},
    pack::{
        CallKindsAt, CallKindsFrom, CallPosAt, CallsAt, CompKindsAt, CompsAt, DescAt, EndReasonOf,
        FkAt, FkOf, IlOf, KindPosAt, NthCall, NthComp, PackNth, RunningAt, RunningCalls, StartAt,
        StartCalls, StartOf,
    },
};

/// The items of the pack at a position of the structure, for the driver.
pub(crate) mod pack_at {
    pub(crate) use super::index::{IxDesc, IxEndReason, IxFk, IxOps, IxStart, ix_table};
}

/// What a composition proves and packs, kind by kind: the items its proof
/// and the driver's lending go through, which
/// [`composition!`](crate::composition!) names. Not for use otherwise.
#[doc(hidden)]
pub mod __pack {
    pub use super::{
        composition::new_root,
        index::PackIndex,
        kinds::{
            AllKinds, AnyIs, Comp, FindAt, FindInKids, FindKind, HasKind, InPart, KidKinds,
            KindsOnce, ListRest, Or, PartsCalls, PartsComps, PickKid, RequestLists, RequestQueues,
        },
        pack::{
            CallList, CompList, Desc, KindDesc, KindList, KindOps, KindVisit, NewAt, NewAtOf,
            NoKind, NoKindPos, PackList, SlotPoll, TableOf,
        },
        prove::{
            FewCalls, Fits, KindPositions, Prove, ProveCalls, ProveChildren, ProveComps, RootBuild,
            prove,
        },
    };
}

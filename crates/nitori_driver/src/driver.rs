//! The driver: the tree of a composition, its calls and its components,
//! run by priority.
//!
//! [`Driver::new`] builds the root of the tree and starts its calls; the
//! driver is a future ([`Driver::poll`](core::future::Future::poll)) that
//! handles what was woken, the highest priority first, until the root
//! ended. A call's standard waker, the one its `.await`s register, wakes
//! it at the priority it set ([`Loan::set_wake_priority`]); a component's
//! waker ([`ComponentWaker`]) wakes it through [`Wakes`] at the priority
//! given there. Each item handled lends once; after it, the driver handles
//! the requests put in during the lending in the order they were put in,
//! then the wakes, then ranks the calls that settled
//! ([`Loan::settle`]), and takes the highest item again.
//!
//! Dropping the driver ends every node, the nodes below before the node
//! above, for [`EndCause::Dropped`]. When a component's operation or a
//! call panics through an entry of the driver, the driver calls no
//! operation of any component after: the next poll is ready, and dropping
//! it drops the tree without ending it.
//!
//! [`Loan::set_wake_priority`]: crate::call::Loan::set_wake_priority
//! [`Loan::settle`]: crate::call::Loan::settle
//! [`ComponentWaker`]: crate::need::ComponentWaker
//! [`Wakes`]: crate::need::Wakes
//! [`EndCause::Dropped`]: crate::component::EndCause::Dropped
use alloc::{sync::Arc, vec::Vec};
use core::{
    future::Future,
    pin::Pin,
    sync::atomic::Ordering::SeqCst,
    task::{Context, Poll, Waker},
};

use nitori_clock::MonotonicClock;

use crate::{
    call::{
        __pack::PackList, ClockOf, Composition, ContextOf, KindsOf, PartsOf, PriorityOf, RootNode,
        RootOf, context,
    },
    component::ArgsOf,
    structure::NodeKind,
};

mod add;
mod edit;
mod end;
mod nodes;
mod operate;
mod place;
mod ready;
mod run;
mod table;
mod visit;
mod wake;

#[cfg(test)]
mod tests;

pub(crate) use add::AddRefused;
#[doc(hidden)]
pub use edit::{
    ChildEditAt, ChildOps, ChildOpsList, ChildPos, FkAppend, FkStep, HandlerAt, NoChildEdit,
    NoChildOps, NthChildOps,
};
pub use edit::{Edit, EditPos, HandlerFor, HandlerPos, NoChildError};
pub use operate::OperateError;
#[doc(hidden)]
pub use place::FkPlace;
#[doc(hidden)]
pub use table::{
    AllLists, AllTables, KindEntry, KindTable, ListsAt, TablesAt, TailLists, TailTables,
};
pub(crate) use wake::NodeHead;

use nodes::Nodes;
use ready::{Item, ReadyQueue};
use wake::Shared;

/// The most items one poll handles: past them the driver wakes its waker
/// and gives way, and the next poll goes on by priority.
///
/// It keeps a call that wakes itself on every poll, or a burst of wakes
/// that keeps coming, from holding the executor's thread: about 128 items
/// stay a short time, while a poll still handles a usual burst of wakes
/// (an item per packet or per stream of one read) without giving way in
/// the middle of it. Requests are not counted: they are handled after the
/// item that put them in, within its turn.
pub const ITEMS_PER_POLL: usize = 128;

/// What the driver of a composition `Q` keeps tables of: what was proved
/// of every kind.
pub(crate) type Tables<Q> = <Q as Composition>::Tables;

/// The driver of a tree of the composition `Q`: a future that runs its
/// calls and components by priority until the root ended.
///
/// A component's operation or a call that panics unwinds through the
/// entry it ran in (`poll`, [`operate`](Driver::operate)) to whoever
/// called it. Nothing of a component runs after that: when the panic is
/// caught, the driver is ready on its next poll, as when the root ended,
/// and [`operate`](Driver::operate) gives
/// [`OperateError::Panicked`]; dropping it ends nothing.
pub struct Driver<Q: Composition> {
    phase: Phase<Q>,
}
// The driver pins nothing of its own: every call is in a box of its own.
impl<Q: Composition> Unpin for Driver<Q> {}

/// Where a driver is.
enum Phase<Q: Composition> {
    /// The tree is there.
    Live(alloc::boxed::Box<Core<Q>>),
    /// The root ended, or the driver was dropped.
    Ended,
    /// A component's operation or a call panicked through an entry: the
    /// tree was dropped without ending it.
    Panicked,
}

/// What a live driver keeps.
pub(crate) struct Core<Q: Composition> {
    /// An entry runs: still set at the next entry, an operation or a call
    /// panicked through the last one.
    pub(crate) running: bool,
    /// The root ended: the driver is done.
    pub(crate) root_ended: bool,
    pub(crate) root: RootNode<Q>,
    /// The context every lending takes, kept for the driver's life.
    pub(crate) cx: ContextOf<Q>,
    /// The clock the driver reads before each lending.
    pub(crate) clock: ClockOf<Q>,
    /// What every new node's components are built with.
    pub(crate) args: ArgsOf<PartsOf<Q>>,
    pub(crate) tables: Tables<Q>,
    pub(crate) nodes: Nodes<PriorityOf<Q>>,
    pub(crate) ready: ReadyQueue<PriorityOf<Q>>,
    /// The calls that settled in the lending being handled, at their
    /// priorities.
    pub(crate) settles: Vec<(Item, PriorityOf<Q>)>,
    /// The requests queued after a lending, each with its place in the
    /// order and its kind; kept for its room.
    pub(crate) order: Vec<(u64, usize)>,
    /// The wakes taken after a lending; kept for its room.
    pub(crate) wakes: Vec<(crate::need::ComponentWaker, PriorityOf<Q>)>,
    pub(crate) shared: Arc<Shared>,
    /// The waker of the last poll.
    pub(crate) outer: Option<Waker>,
    /// How many outer wakers the driver was polled with.
    pub(crate) epoch: u64,
    /// The requests handled: each one's place in the order, its kind, and
    /// whether its node was there.
    #[cfg(test)]
    pub(crate) trace: Vec<(u64, usize, bool)>,
}

impl<Q: Composition> Driver<Q> {
    /// A driver of a new tree: its root built from `start` and the
    /// arguments `args` of the parts, which every node is built with, and
    /// read on `clock`. Every call of the root is woken at `p` first.
    pub fn new(
        clock: ClockOf<Q>,
        mut start: <RootOf<Q> as NodeKind>::Start,
        args: ArgsOf<PartsOf<Q>>,
        p: PriorityOf<Q>,
    ) -> Self {
        let now = clock.now();
        let root = Q::root(&mut start, &args);
        let mut core = alloc::boxed::Box::new(Core {
            running: true,
            root_ended: false,
            root,
            cx: context::<Q>(clock.clone(), now),
            clock,
            args,
            tables: <Q::Pack as PackList<Q, KindsOf<Q>>>::tables(),
            nodes: Nodes::new(),
            ready: ReadyQueue::new(),
            settles: Vec::new(),
            order: Vec::new(),
            wakes: Vec::new(),
            shared: Arc::new(Shared::default()),
            outer: None,
            epoch: 0,
            #[cfg(test)]
            trace: Vec::new(),
        });
        core.add_root(start, p);
        core.after_lend();
        let mut driver = Self {
            phase: Phase::Live(core),
        };
        // The root may have ended in what its start put in: the driver is
        // done then.
        driver.leave();
        driver
    }

    /// The live core, for an entry: `None` once the driver is done, or
    /// when the last entry panicked, which the driver then records.
    fn enter(&mut self) -> Option<&mut Core<Q>> {
        let Phase::Live(core) = &mut self.phase else {
            return None;
        };
        if core.running {
            core.shared.closed.store(true, SeqCst);
            self.phase = Phase::Panicked;
            return None;
        }
        // Invariant: matched just above.
        let Phase::Live(core) = &mut self.phase else {
            unreachable!("the driver is live")
        };
        core.running = true;
        Some(core)
    }

    /// Leave an entry: done when the root ended.
    fn leave(&mut self) {
        let Phase::Live(core) = &mut self.phase else {
            return;
        };
        core.running = false;
        if core.root_ended {
            core.shared.closed.store(true, SeqCst);
            self.phase = Phase::Ended;
        }
    }
}

impl<Q: Composition> Future for Driver<Q> {
    type Output = ();
    /// Handle what was woken, the highest priority first, at most
    /// [`ITEMS_PER_POLL`] items; ready once the root ended, or after a
    /// panic through an entry.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let Some(core) = this.enter() else {
            return Poll::Ready(());
        };
        let polled = core.poll_round(cx.waker());
        this.leave();
        match this.phase {
            Phase::Live(_) => polled,
            Phase::Ended | Phase::Panicked => Poll::Ready(()),
        }
    }
}

impl<Q: Composition> Drop for Driver<Q> {
    /// End every node, the nodes below before the node above, for
    /// [`EndCause::Dropped`](crate::component::EndCause::Dropped); the
    /// requests and wakes put in meanwhile are dropped. After a panic
    /// through an entry, nothing is ended: the tree is dropped as it is.
    fn drop(&mut self) {
        let Some(core) = self.enter() else {
            return;
        };
        core.end_dropped();
        self.leave();
    }
}

//! Waking without a lock: a mark for each position in a node's head, and
//! a stack of the nodes marked since the driver last took them.
//!
//! A waker sets its bit in its node's marks; the node whose marks were
//! empty goes on the ready stack, a list linked through the nodes' heads
//! by node number, pushed with a compare-and-swap and taken whole with a
//! swap. Then the outer waker the waker was made with is woken, unless the
//! outer waker woken last since the poll began is the same one.
//!
//! The outer waker is not kept where wakers on other threads read it: a
//! position's waker holds the outer waker of the poll it was made in, and
//! the driver makes them again, and polls every call once, when it is
//! polled with another outer waker. Which outer waker was woken is kept by
//! its number (its epoch): a resource may still wake a waker of an older
//! outer waker, which wakes that one and leaves the current one to be
//! woken by its own wakers. Nothing here needs a lock or `unsafe`.
//!
//! A node's number is not given to another node while a waker of the old
//! one is alive or the old one is on the stack: its head stays in the
//! driver's table, retired, until then. A waker of a node that ended marks
//! the retired head and wakes nothing.
use alloc::{sync::Arc, task::Wake, vec::Vec};
use core::{
    sync::atomic::{
        AtomicBool, AtomicU64, AtomicUsize,
        Ordering::{Acquire, SeqCst},
        fence,
    },
    task::Waker,
};

use frunk_core::indices::Here as I0;

use super::{
    Driver, DriverComposition, NodeSlot, NthPlace, Place, PlaceNth, PlacesOf, Ready,
    walk::{CallWalk, Cursor, PlaceWalk},
};
use crate::{
    call::{PartsOf, RootOf},
    structure::{ChildAddresses, End, NodeKind, RemainsOf},
};

/// [`Shared::notified`] when no outer waker was woken since the poll
/// began.
pub(crate) const NOT_NOTIFIED: u64 = u64::MAX;

/// What every waker of a driver shares.
#[derive(Debug)]
pub(crate) struct Shared {
    /// The ready stack: the number of the node on top, plus one; 0 when
    /// empty.
    top: AtomicUsize,
    /// The epoch of the outer waker woken last since this poll began;
    /// [`NOT_NOTIFIED`] when none was.
    pub(crate) notified: AtomicU64,
    /// The driver ended: wakes do nothing.
    pub(crate) closed: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            top: AtomicUsize::new(0),
            notified: AtomicU64::new(NOT_NOTIFIED),
            closed: AtomicBool::new(false),
        }
    }
}

/// An outer waker and its epoch: the driver's count of the outer wakers it
/// was polled with, when it was polled with this one.
#[derive(Clone, Copy)]
pub(crate) struct Outer<'w> {
    pub(crate) waker: &'w Waker,
    pub(crate) epoch: u64,
}

/// The head of a node: its marks and its link in the ready stack.
#[derive(Debug)]
pub(crate) struct NodeHead {
    pub(crate) id: usize,
    pub(crate) marks: AtomicU64,
    next: AtomicUsize,
    pub(crate) dead: AtomicBool,
}
impl NodeHead {
    pub(crate) fn new(id: usize) -> Self {
        Self {
            id,
            marks: AtomicU64::new(0),
            next: AtomicUsize::new(0),
            dead: AtomicBool::new(false),
        }
    }
    /// Whether nothing refers to the head but the driver's table, and it
    /// is not on the stack: its number can go to another node.
    pub(crate) fn reusable(head: &Arc<Self>) -> bool {
        if Arc::strong_count(head) != 1 {
            return false;
        }
        // `strong_count` is a relaxed read. A waker that marked the head
        // did so (`fetch_or`) before it dropped its `Arc`, which releases;
        // this fence makes that drop, seen through the count of one,
        // happen before the read of the marks below, so a mark made before
        // the last waker went away is seen and the number is not given out
        // while the node is still on the stack.
        fence(Acquire);
        head.marks.load(SeqCst) == 0
    }
}

impl Shared {
    /// Mark `bits` of `head`'s node; a node whose marks were empty goes on
    /// the stack. Then wake `outer`, unless the outer waker woken last
    /// since the poll began is of the same epoch.
    ///
    /// A wake through an older outer waker does not keep the current one
    /// from being woken: it puts its own epoch, so the next wake through
    /// the current one sees another epoch and wakes again.
    pub(crate) fn mark(&self, head: &NodeHead, bits: u64, outer: Option<Outer<'_>>) {
        if self.closed.load(SeqCst) || head.dead.load(SeqCst) || bits == 0 {
            return;
        }
        if head.marks.fetch_or(bits, SeqCst) == 0 {
            let mut top = self.top.load(SeqCst);
            loop {
                head.next.store(top, SeqCst);
                match self
                    .top
                    .compare_exchange_weak(top, head.id + 1, SeqCst, SeqCst)
                {
                    Ok(_) => break,
                    Err(now) => top = now,
                }
            }
        }
        if let Some(outer) = outer
            && self.notified.swap(outer.epoch, SeqCst) != outer.epoch
        {
            outer.waker.wake_by_ref();
        }
    }
    /// Take the whole stack: the numbers of the nodes marked, in the order
    /// they were first marked. `head` gives the head of a number, live or
    /// retired.
    pub(crate) fn take<'h>(
        &self,
        head: impl Fn(usize) -> Option<&'h NodeHead>,
    ) -> alloc::vec::Vec<usize> {
        let mut at = self.top.swap(0, SeqCst);
        let mut out = alloc::vec::Vec::new();
        while at != 0 {
            let id = at - 1;
            out.push(id);
            // Invariant: a number on the stack keeps its head in the table
            // until it is taken off ([`NodeHead::reusable`]).
            at = head(id).map_or(0, |h| h.next.load(SeqCst));
        }
        out.reverse();
        out
    }
}

/// The waker of one position of a node: a call, or a component that acts
/// before the calls.
pub(crate) struct PositionWaker {
    pub(crate) shared: Arc<Shared>,
    pub(crate) head: Arc<NodeHead>,
    pub(crate) bit: u64,
    pub(crate) outer: Waker,
    /// The epoch of `outer`.
    pub(crate) epoch: u64,
}
impl Wake for PositionWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.shared.mark(
            &self.head,
            self.bit,
            Some(Outer {
                waker: &self.outer,
                epoch: self.epoch,
            }),
        );
    }
}

impl<Q: DriverComposition> Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
    /// The waker of the position `bit` of the node `id`, made with the
    /// current outer waker.
    pub(super) fn waker(&mut self, id: usize, bit: usize) -> Waker {
        let epoch = self.epoch;
        let outer = self.outer.clone().unwrap_or_else(|| Waker::noop().clone());
        let shared = self.shared.clone();
        let NodeSlot::Live(n) = &mut self.nodes[id] else {
            return Waker::noop().clone();
        };
        match &n.wakers[bit] {
            Some((e, w)) if *e == epoch => w.clone(),
            _ => {
                let w = Waker::from(Arc::new(PositionWaker {
                    shared,
                    head: n.head.clone(),
                    bit: 1 << bit,
                    outer,
                    epoch,
                }));
                n.wakers[bit] = Some((epoch, w.clone()));
                w
            }
        }
    }
    pub(super) fn mark(&self, id: usize, bits: u64) {
        if let Some(n) = self.live(id) {
            let outer = self.outer.as_ref().map(|waker| Outer {
                waker,
                epoch: self.epoch,
            });
            self.shared.mark(&n.head, bits, outer);
        }
    }
    /// Take the nodes marked, with their marks, in the order of their
    /// first mark.
    /// The nodes marked, each with its serial and the bits marked.
    pub(super) fn take_ready(&mut self) -> Vec<Ready> {
        let ids = self
            .shared
            .take(|id| self.nodes.get(id).and_then(NodeSlot::head));
        let mut out = Vec::new();
        for id in ids {
            let Some(head) = self.nodes.get(id).and_then(NodeSlot::head) else {
                continue;
            };
            let bits = head.marks.swap(0, SeqCst);
            if let Some(n) = self.live(id)
                && bits != 0
            {
                out.push(Ready {
                    node: id,
                    serial: n.serial,
                    bits,
                });
            }
        }
        out
    }
}

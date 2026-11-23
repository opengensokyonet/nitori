//! Waking without a lock: a mark for each position in a node's head, and
//! a stack of the nodes marked since the driver last took them.
//!
//! A waker sets its bit in its node's marks; the node whose marks were
//! empty goes on the ready stack, a list linked through the nodes' heads
//! by node number, pushed with a compare-and-swap and taken whole with a
//! swap. Then, the first time since the poll began, the outer waker the
//! waker was made with is woken.
//!
//! The outer waker is not kept where wakers on other threads read it: a
//! position's waker holds the outer waker of the poll it was made in, and
//! the driver makes them again, and polls every call once, when it is
//! polled with another outer waker. Nothing here needs a lock or `unsafe`.
//!
//! A node's number is not given to another node while a waker of the old
//! one is alive or the old one is on the stack: its head stays in the
//! driver's table, retired, until then. A waker of a node that ended marks
//! the retired head and wakes nothing.
use alloc::{sync::Arc, task::Wake};
use core::{
    sync::atomic::{
        AtomicBool, AtomicU64, AtomicUsize,
        Ordering::{Acquire, SeqCst},
        fence,
    },
    task::Waker,
};

/// What every waker of a driver shares.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// The ready stack: the number of the node on top, plus one; 0 when
    /// empty.
    top: AtomicUsize,
    /// Whether the outer waker was woken since this poll began.
    pub(crate) notified: AtomicBool,
    /// The driver ended: wakes do nothing.
    pub(crate) closed: AtomicBool,
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
    /// the stack. Then wake `outer`, the first time since the poll began.
    pub(crate) fn mark(&self, head: &NodeHead, bits: u64, outer: Option<&Waker>) {
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
            && !self.notified.swap(true, SeqCst)
        {
            outer.wake_by_ref();
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
}
impl Wake for PositionWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.shared.mark(&self.head, self.bit, Some(&self.outer));
    }
}

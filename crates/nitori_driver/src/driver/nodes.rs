//! The nodes a driver keeps, by number: their kind, their head, their
//! place among the others, and what the driver keeps of their calls.
use alloc::{sync::Arc, vec::Vec};
use core::task::Waker;

use super::wake::NodeHead;
use crate::call::CallHost;

/// A node the driver keeps.
pub(crate) struct LiveNode<P> {
    /// The number of its kind, in the order of the composition's kinds.
    pub(crate) kind: usize,
    /// Its serial ([`NodeIdent`](crate::structure::NodeIdent)).
    pub(crate) serial: u64,
    pub(crate) head: Arc<NodeHead>,
    pub(crate) parent: Option<u32>,
    pub(crate) children: Vec<u32>,
    /// What the driver keeps of each of its calls, by place.
    pub(crate) hosts: Vec<CallHost<P>>,
    /// The waker of each of its calls, with the epoch it was made in.
    pub(crate) wakers: Vec<Option<(u64, Waker)>>,
}
impl<P> LiveNode<P> {
    /// The bits of every call.
    pub(crate) fn all_calls(&self) -> u64 {
        // Invariant: a kind has at most 64 calls (`FewCalls`).
        match self.hosts.len() {
            64 => u64::MAX,
            n => (1u64 << n) - 1,
        }
    }
}

/// A number of the table.
pub(crate) enum NodeSlot<P> {
    /// Free: given to the next node.
    Free,
    /// A node kept under it.
    Live(LiveNode<P>),
    /// Of a node that ended, its head still referred to by a waker or on
    /// the ready stack.
    Retired(Arc<NodeHead>),
}
impl<P> NodeSlot<P> {
    /// The head of the number, live or retired.
    pub(crate) fn head(&self) -> Option<&NodeHead> {
        match self {
            Self::Free => None,
            Self::Live(n) => Some(&n.head),
            Self::Retired(h) => Some(h),
        }
    }
}

/// The table of numbers.
pub(crate) struct Nodes<P> {
    pub(crate) slots: Vec<NodeSlot<P>>,
}
impl<P> Nodes<P> {
    pub(crate) fn new() -> Self {
        Self { slots: Vec::new() }
    }
    /// The node kept under `number`.
    pub(crate) fn live(&self, number: u32) -> Option<&LiveNode<P>> {
        match self.slots.get(usize::try_from(number).ok()?)? {
            NodeSlot::Live(n) => Some(n),
            _ => None,
        }
    }
    /// The node kept under `number`, exclusive.
    pub(crate) fn live_mut(&mut self, number: u32) -> Option<&mut LiveNode<P>> {
        match self.slots.get_mut(usize::try_from(number).ok()?)? {
            NodeSlot::Live(n) => Some(n),
            _ => None,
        }
    }
    /// A number to give a new node: a free one, one whose retired head
    /// nothing refers to any more, or a new one.
    pub(crate) fn free_number(&mut self) -> u32 {
        let at = self.slots.iter().position(|s| match s {
            NodeSlot::Free => true,
            NodeSlot::Live(_) => false,
            NodeSlot::Retired(h) => NodeHead::reusable(h),
        });
        let at = at.unwrap_or_else(|| {
            self.slots.push(NodeSlot::Free);
            self.slots.len() - 1
        });
        // Invariant: a driver keeps fewer than 2³² nodes at once; a tree
        // that big does not fit in memory first.
        u32::try_from(at).expect("fewer than 2^32 nodes")
    }
    /// Keep `node` under `number`.
    pub(crate) fn put(&mut self, number: u32, node: LiveNode<P>) {
        // Invariant: the number came from `free_number`.
        self.slots[number as usize] = NodeSlot::Live(node);
    }
    /// The node kept under `number` ended: its number is retired until
    /// nothing refers to its head.
    pub(crate) fn retire(&mut self, number: u32) -> Option<LiveNode<P>> {
        let slot = self.slots.get_mut(number as usize)?;
        let NodeSlot::Live(n) = core::mem::replace(slot, NodeSlot::Free) else {
            return None;
        };
        n.head
            .dead
            .store(true, core::sync::atomic::Ordering::SeqCst);
        *slot = NodeSlot::Retired(n.head.clone());
        Some(n)
    }
    /// Every node kept, with its number.
    pub(crate) fn iter_live(&self) -> impl Iterator<Item = (u32, &LiveNode<P>)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| match s {
            // Invariant: as for `free_number`.
            NodeSlot::Live(n) => Some((i as u32, n)),
            _ => None,
        })
    }
}

//! The ready queue: what was woken, ranked by priority, each once.
use alloc::{collections::BTreeMap, vec::Vec};
use core::cmp::Reverse;

/// What is woken: a call of a node, by its place among the node's calls,
/// or a component of a node, by its number.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Target {
    /// The call at this place.
    Call(u32),
    /// The component with this number.
    Component(u64),
}

/// One item of the queue: a target at the node kept under a number.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Item {
    pub(crate) node: u32,
    pub(crate) target: Target,
}

/// The items woken and not handled yet, the highest priority first, and
/// among one priority the one queued there first.
///
/// An item woken again before it is handled stays once, at the highest
/// priority it was woken at; raised, it is queued again at the new
/// priority, behind what was queued there before.
pub(crate) struct ReadyQueue<P> {
    order: BTreeMap<(Reverse<P>, u64), Item>,
    items: BTreeMap<Item, (P, u64)>,
    next: u64,
}
impl<P: Ord + Copy> ReadyQueue<P> {
    pub(crate) fn new() -> Self {
        Self {
            order: BTreeMap::new(),
            items: BTreeMap::new(),
            next: 0,
        }
    }
    /// Wake `item` at `p`.
    pub(crate) fn wake(&mut self, item: Item, p: P) {
        if let Some(&(was, seq)) = self.items.get(&item) {
            if was >= p {
                return;
            }
            self.order.remove(&(Reverse(was), seq));
        }
        let seq = self.next;
        // A count of the wakes of one driver: 2⁶⁴ of them are not reached.
        self.next += 1;
        self.order.insert((Reverse(p), seq), item);
        self.items.insert(item, (p, seq));
    }
    /// The item to handle next, taken off the queue, with its priority.
    pub(crate) fn pop(&mut self) -> Option<(Item, P)> {
        let ((Reverse(p), _), item) = self.order.pop_first()?;
        self.items.remove(&item);
        Some((item, p))
    }
    /// Whether nothing is queued.
    pub(crate) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    /// Take every item of the node `node` off the queue.
    pub(crate) fn remove_node(&mut self, node: u32) {
        let first = Item {
            node,
            target: Target::Call(0),
        };
        let last = Item {
            node,
            target: Target::Component(u64::MAX),
        };
        let gone: Vec<(Item, (P, u64))> = self
            .items
            .range(first..=last)
            .map(|(i, at)| (*i, *at))
            .collect();
        for (item, (p, seq)) in gone {
            self.items.remove(&item);
            self.order.remove(&(Reverse(p), seq));
        }
    }
    /// Take everything off the queue.
    pub(crate) fn clear(&mut self) {
        self.order.clear();
        self.items.clear();
    }
}

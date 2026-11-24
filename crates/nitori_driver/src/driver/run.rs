//! A poll of the driver: what was woken, handled by priority, and after
//! each item the requests, the wakes and the calls that settled.
use alloc::sync::Arc;
use core::{
    sync::atomic::Ordering::SeqCst,
    task::{Context, Poll, Waker},
};

use nitori_clock::MonotonicClock;

use super::{
    AllLists, AllTables, Core, ITEMS_PER_POLL,
    nodes::NodeSlot,
    ready::{Item, Target},
    visit::{PollCall, WakeComponent},
    wake::{CallWaker, NOT_NOTIFIED},
};
use crate::call::{
    __pack::{PackList, RequestQueues, SlotPoll},
    Composition, KindsOf, request_lists, set_now, swap_wakes,
};
#[cfg(test)]
use crate::structure::resolve_node;

impl<Q: Composition> Core<Q> {
    /// One poll with the outer waker `waker`: at most [`ITEMS_PER_POLL`]
    /// items, the highest priority first; ready once the root ended.
    pub(super) fn poll_round(&mut self, waker: &Waker) -> Poll<()> {
        if self.outer.as_ref().is_none_or(|w| !w.will_wake(waker)) {
            // A waker of the old outer waker would wake only that one:
            // every call is polled again, with a waker of the new one.
            self.epoch += 1;
            self.outer = Some(waker.clone());
            for (_, n) in self.nodes.iter_live() {
                self.shared.mark(&n.head, n.all_calls(), None);
            }
        }
        self.shared.notified.store(NOT_NOTIFIED, SeqCst);
        let mut handled = 0;
        loop {
            self.take_marks();
            if self.ready.is_empty() {
                return Poll::Pending;
            }
            if handled == ITEMS_PER_POLL {
                waker.wake_by_ref();
                return Poll::Pending;
            }
            let Some((item, _)) = self.ready.pop() else {
                return Poll::Pending;
            };
            handled += 1;
            self.read_clock();
            self.handle(item);
            self.after_lend();
            if self.root_ended {
                return Poll::Ready(());
            }
        }
    }

    /// Read the clock for the lendings that follow.
    pub(super) fn read_clock(&mut self) {
        let now = self.clock.now();
        set_now::<Q>(&mut self.cx, now);
    }

    /// Queue every call marked by its waker since the last time, at the
    /// priority it set.
    fn take_marks(&mut self) {
        let slots = &self.nodes.slots;
        let mut ids = core::mem::take(&mut self.marked);
        self.shared
            .take(|id| slots.get(id).and_then(NodeSlot::head), &mut ids);
        for &id in &ids {
            let Some(head) = self.nodes.slots.get(id).and_then(NodeSlot::head) else {
                continue;
            };
            let bits = head.marks.swap(0, SeqCst);
            // Invariant: a number is less than 2³² (`Nodes::free_number`).
            let number = id as u32;
            let Some(n) = self.nodes.live(number) else {
                continue;
            };
            for (j, host) in n.hosts.iter().enumerate() {
                if bits & (1 << j) != 0 {
                    let item = Item {
                        node: number,
                        // Invariant: at most 64 calls.
                        target: Target::Call(j as u32),
                    };
                    self.ready.wake(item, host.wake_priority());
                }
            }
        }
        self.marked = ids;
    }

    /// The waker of the call `j` of the node `number`, made with the
    /// current outer waker.
    fn call_waker(&mut self, number: u32, j: u32) -> Option<Waker> {
        let epoch = self.epoch;
        let n = self.nodes.live_mut(number)?;
        let slot = n.wakers.get_mut(j as usize)?;
        if let Some((e, w)) = slot
            && *e == epoch
        {
            return Some(w.clone());
        }
        let outer = self.outer.clone().unwrap_or_else(|| Waker::noop().clone());
        let w = Waker::from(Arc::new(CallWaker {
            shared: self.shared.clone(),
            head: n.head.clone(),
            bit: 1 << j,
            outer,
            epoch,
        }));
        *slot = Some((epoch, w.clone()));
        Some(w)
    }

    /// Handle one item: poll the call, or wake the component.
    fn handle(&mut self, item: Item) {
        let Some(kind) = self.nodes.live(item.node).map(|n| n.kind) else {
            return;
        };
        match item.target {
            Target::Call(j) => {
                let Some(waker) = self.call_waker(item.node, j) else {
                    return;
                };
                let mut wcx = Context::from_waker(&waker);
                let Self {
                    root,
                    cx,
                    tables,
                    nodes,
                    settles,
                    ..
                } = self;
                let Some(host) = nodes
                    .live_mut(item.node)
                    .and_then(|n| n.hosts.get_mut(j as usize))
                else {
                    return;
                };
                let polled = <Q::Pack as PackList<Q, KindsOf<Q>>>::visit(
                    tables,
                    kind,
                    PollCall {
                        root,
                        cx,
                        number: item.node,
                        j,
                        host,
                        wcx: &mut wcx,
                    },
                );
                if polled == Some(SlotPoll::Pending)
                    && let Some(p) = host.take_settle()
                {
                    settles.push((item, p));
                }
            }
            Target::Component(component) => {
                let Self {
                    root, cx, tables, ..
                } = self;
                <Q::Pack as PackList<Q, KindsOf<Q>>>::visit(
                    tables,
                    kind,
                    WakeComponent {
                        root,
                        cx,
                        number: item.node,
                        component,
                    },
                );
            }
        }
    }

    /// After a lending: the requests put in, in the order they were put
    /// in across every kind, each handed to its kind's handler, then the
    /// wakes, until neither was put in again; then the calls that settled,
    /// each at its priority.
    ///
    /// A request for a node that is not there is dropped. What a handler
    /// puts in is handled after the requests put in before it.
    ///
    /// The boxes, the order and the wakes keep their room from one lending
    /// to the next.
    pub(super) fn after_lend(&mut self) {
        let mut order = core::mem::take(&mut self.order);
        let mut wakes = core::mem::take(&mut self.wakes);
        loop {
            // What a handler puts in stays in its box until the next
            // round: only what is queued here is handled in this one.
            order.clear();
            request_lists::<Q>(&mut self.cx).enqueue_all(0, &mut order);
            order.sort_unstable();
            let mut any = !order.is_empty();
            for &(_place, kind) in &order {
                #[cfg(test)]
                {
                    let there = request_lists::<Q>(&mut self.cx)
                        .first(kind)
                        .is_some_and(|(_, route)| resolve_node(&self.root, route).is_some());
                    self.trace.push((_place, kind, there));
                }
                <Q::Pack as PackList<Q, KindsOf<Q>>>::handle_first::<AllTables, AllLists>(
                    self, kind,
                );
            }
            swap_wakes::<Q>(&mut self.cx, &mut wakes);
            any |= !wakes.is_empty();
            for (waker, p) in wakes.drain(..) {
                let node = waker.node();
                if self
                    .nodes
                    .live(node.number)
                    .is_some_and(|n| n.serial == node.serial)
                {
                    let item = Item {
                        node: node.number,
                        target: Target::Component(waker.component()),
                    };
                    self.ready.wake(item, p);
                }
            }
            if !any {
                break;
            }
        }
        self.order = order;
        self.wakes = wakes;
        let mut settles = core::mem::take(&mut self.settles);
        for (item, p) in settles.drain(..) {
            if self.nodes.live(item.node).is_some() {
                self.ready.wake(item, p);
            }
        }
        self.settles = settles;
    }
}

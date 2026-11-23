//! One poll of the driver, and the driver as a future.
use alloc::{boxed::Box, collections::BTreeSet, vec::Vec};
use core::{
    future::Future,
    pin::Pin,
    sync::atomic::Ordering::SeqCst,
    task::{Context, Poll, Waker},
};

use frunk_core::indices::Here as I0;
use nitori_clock::MonotonicClock;

use super::{
    AddressOf, Driver, DriverComposition, InstantOf, NthPlace, Place, PlaceDesc, PlaceNth,
    PlaceOps, PlacesOf, Ready, RequestsOf, RootNode, Target, edit,
    walk::{CallWalk, Cursor, PlaceVisit, PlaceWalk, SlotStore, SlotVisit},
};
use crate::{
    call::{CallStep, LendOne, OutboxOf, PartsOf, RootOf, SlotCall, SlotDesc, SlotYield},
    structure::{ChildAddresses, End, FromAddress, NodeKind, RemainsOf},
};

struct BeforeCalls<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    c: usize,
    waker: &'r Waker,
    outbox: &'r OutboxOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for BeforeCalls<'_, Q> {
    type Out = Option<RequestsOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::before_calls(self.root, self.at, self.c, self.waker, self.outbox)
    }
}
struct Expire<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    c: usize,
    now: InstantOf<Q>,
    outbox: &'r OutboxOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for Expire<'_, Q> {
    type Out = Option<RequestsOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::expire(self.root, self.at, self.c, self.now, self.outbox)
    }
}
/// What polling a call came to, as the driver goes on from it.
enum Polled {
    /// Waits, or is not there.
    Pending,
    /// Over, or no longer needed: dropped.
    Dropped,
    /// Yielded and goes on: it is polled again.
    Yielded,
}
struct PollCall<'r, 'c, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    node: usize,
    at: &'r AddressOf<Q>,
    outbox: &'r OutboxOf<Q>,
    cx: &'r mut Context<'c>,
}
impl<Q: DriverComposition> SlotVisit<Q> for PollCall<'_, '_, Q> {
    type Out = Polled;
    fn visit<S, Pos>(self, store: &mut SlotStore<Pos::Running, S::Yield, AddressOf<Q>>) -> Polled
    where
        S: SlotDesc<At: FromAddress<RootOf<Q>> + 'static> + SlotYield<Q>,
        Pos: SlotCall<Q, S> + for<'a> LendOne<'a, Q, S>,
    {
        let Some(at) = S::At::from_address(self.at) else {
            return Polled::Pending;
        };
        let Some(Some(call)) = store.calls.get_mut(self.node) else {
            return Polled::Pending;
        };
        match Pos::poll(self.root, at, self.outbox, call.as_mut(), self.cx) {
            Ok(CallStep::Pending) => Polled::Pending,
            Err(_) => {
                // Invariant: a call is polled only at a node the driver
                // has, so its lend finds the node.
                debug_assert!(false, "a call is lent at a node the driver has");
                Polled::Pending
            }
            Ok(CallStep::Finished | CallStep::NoLongerNeeded) => {
                store.calls[self.node] = None;
                Polled::Dropped
            }
            Ok(CallStep::Yielded(item)) => {
                store.yields.push(item);
                Polled::Yielded
            }
        }
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
    RequestsOf<Q>: edit::Deliver<Q, I0>,
{
    fn turn(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.ended {
            return Poll::Ready(());
        }
        if !self.outer.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
            // the wakers made with the old outer waker wake it, not this
            // one: every position is polled once and gets a new waker
            self.outer = Some(cx.waker().clone());
            self.epoch += 1;
            for id in self.live_ids().collect::<Vec<_>>() {
                let p = self.nodes[id].live().map_or(0, |n| n.place);
                let all = self.places[p].calls_mask() | self.places[p].before_mask();
                if let Some(n) = self.live(id) {
                    self.shared.mark(&n.head, all, None);
                }
            }
        }
        self.shared.notified.store(false, SeqCst);
        let now = self.clock.now();
        self.outbox.set_now(now);
        let deferred = core::mem::take(&mut self.deferred);
        self.settle(deferred);
        if self.ended {
            return Poll::Ready(());
        }
        // the actions before the calls, the nodes above first; a node
        // ended or replaced since it was marked is passed over, by its
        // serial, as a hit is
        let mut ready = self.take_ready();
        let mut before: Vec<Ready> = ready
            .iter()
            .map(|r| {
                let p = self.nodes[r.node].live().map_or(0, |n| n.place);
                Ready {
                    bits: r.bits & self.places[p].before_mask(),
                    ..*r
                }
            })
            .filter(|r| r.bits != 0)
            .collect();
        before.sort_by_key(|r| self.depth(r.node));
        for Ready {
            node: id,
            serial,
            bits,
        } in before
        {
            let Some(p) = self.live_as(id, serial).map(|n| n.place) else {
                continue;
            };
            let ncalls = self.places[p].calls.len();
            for j in 0..self.places[p].before.len() {
                if bits & (1 << (ncalls + j)) == 0 || self.ended {
                    continue;
                }
                let Some(addr) = self.live_as(id, serial).map(|n| n.addr) else {
                    break;
                };
                let c = self.places[p].before[j];
                let waker = self.waker(id, ncalls + j);
                let Some(root) = self.root.as_mut() else {
                    return Poll::Ready(());
                };
                let request = <Cursor<I0> as PlaceWalk<Q>>::visit(
                    p,
                    BeforeCalls::<Q> {
                        root,
                        at: &addr,
                        c,
                        waker: &waker,
                        outbox: &self.outbox,
                    },
                )
                .flatten();
                let writes = self.places[p].info.comps[c].writes.clone();
                self.after_lend(id, &writes, None, request);
            }
        }
        if self.ended {
            return Poll::Ready(());
        }
        // what the actions before the calls woke is polled in this poll;
        // an action before the calls woken since runs in the next poll
        for r in self.take_ready() {
            let p = self.nodes[r.node].live().map_or(0, |n| n.place);
            let again = r.bits & self.places[p].before_mask();
            self.mark(r.node, again);
            let calls = r.bits & self.places[p].calls_mask();
            match ready
                .iter_mut()
                .find(|q| (q.node, q.serial) == (r.node, r.serial))
            {
                Some(q) => q.bits |= calls,
                None => ready.push(Ready { bits: calls, ..r }),
            }
        }
        for Ready {
            node: id,
            serial,
            bits,
        } in ready
        {
            let Some(p) = self.live_as(id, serial).map(|n| n.place) else {
                continue;
            };
            let mut bits = bits & self.places[p].calls_mask();
            while bits != 0 {
                let k = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                // a request may have ended the node, replaced it or moved
                // it
                let Some(addr) = self.live_as(id, serial).map(|n| n.addr) else {
                    break;
                };
                let Some(p) = self.live_as(id, serial).map(|n| n.place) else {
                    break;
                };
                let s = self.places[p].calls[k];
                let waker = self.waker(id, k);
                let mut call_cx = Context::from_waker(&waker);
                let Some(root) = self.root.as_mut() else {
                    return Poll::Ready(());
                };
                let polled = <Cursor<I0> as CallWalk<Q>>::visit(
                    s,
                    &mut self.calls,
                    PollCall::<Q> {
                        root,
                        node: id,
                        at: &addr,
                        outbox: &self.outbox,
                        cx: &mut call_cx,
                    },
                );
                if let Some(Polled::Yielded) = polled {
                    self.mark(id, 1 << k);
                }
                let mut requests = Vec::new();
                <Cursor<I0> as CallWalk<Q>>::take_requests(s, &mut self.calls, &mut requests);
                let writes = self.slots[s].writes.clone();
                self.after_lend(id, &writes, Some((id, Target::Call(k))), requests);
                if self.ended {
                    return Poll::Ready(());
                }
            }
        }
        // the deadlines that came, last; each component expires at most once
        // a poll, so a deadline that did not move past `now` comes again in
        // the next poll instead of looping here
        let mut expired = BTreeSet::new();
        while let Some(&(at, id, c)) = self.timers.first() {
            if at > now {
                break;
            }
            self.timers.pop_first();
            let serial = self.live(id).map_or(0, |n| n.serial);
            if !expired.insert((id, serial, c)) {
                self.deferred_timers.push((at, id, c));
                continue;
            }
            self.deadlines.remove(&(id, c));
            if self.live(id).is_none() {
                continue;
            }
            match self.deadline(id, c) {
                Some(current) if current <= now => {
                    let Some(n) = self.live(id) else { continue };
                    let (p, addr) = (n.place, n.addr);
                    let Some(root) = self.root.as_mut() else {
                        return Poll::Ready(());
                    };
                    let request = <Cursor<I0> as PlaceWalk<Q>>::visit(
                        p,
                        Expire::<Q> {
                            root,
                            at: &addr,
                            c,
                            now,
                            outbox: &self.outbox,
                        },
                    )
                    .flatten();
                    let mut todo = Vec::new();
                    let writes = self.places[p].info.comps[c].writes.clone();
                    self.effects(id, &writes, None, &mut todo);
                    if self.live(id).is_some() {
                        self.read_deadline(id, c);
                        debug_assert!(
                            self.deadlines
                                .get(&(id, c))
                                .is_none_or(|&later| later > now),
                            "after its expiry a deadline is later than the instant it expired at, or gone",
                        );
                    }
                    if let Some(request) = request {
                        self.handle(request, &mut todo);
                    }
                    self.settle(todo);
                    if self.ended {
                        return Poll::Ready(());
                    }
                }
                later => self.set_deadline(id, c, later),
            }
        }
        for t in core::mem::take(&mut self.deferred_timers) {
            self.timers.insert(t);
        }
        #[cfg(debug_assertions)]
        self.check_deadlines();
        if let Some(&(earliest, _, _)) = self.timers.first() {
            if self.sleep.as_ref().is_none_or(|(at, _)| earliest < *at) {
                self.sleep = Some((earliest, Box::pin(self.clock.sleep_until(earliest))));
            }
        } else {
            self.sleep = None;
        }
        if let Some((_, sleep)) = self.sleep.as_mut()
            && sleep.as_mut().poll(cx).is_ready()
        {
            self.sleep = None;
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }
}

impl<Q: DriverComposition> Future for Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
    RequestsOf<Q>: edit::Deliver<Q, I0>,
{
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let driver = self.get_mut();
        if driver.running {
            // a panic unwound through the last poll
            driver.end_dropped();
            return Poll::Ready(());
        }
        driver.running = true;
        let turned = driver.turn(cx);
        driver.running = false;
        turned
    }
}

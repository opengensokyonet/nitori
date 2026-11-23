//! Ending nodes: the root ([`Driver::end`]), a node and the nodes below it
//! ([`Driver::remove`]), and a node no node links to any more.
use alloc::vec::Vec;
use core::{any::TypeId, sync::atomic::Ordering::SeqCst};

use frunk_core::indices::Here as I0;
use nitori_clock::MonotonicClock;

use super::{
    AddressOf, Driver, DriverComposition, FindPlace, Hit, NodeSlot, NthPlace, Place, PlaceDesc,
    PlaceKey, PlaceNth, PlaceOps, PlacePositionsAt, PlacesOf, RemoveError, RequestsOf, RootNode,
    TargetOf, edit, place,
    walk::{CallWalk, Cursor, PlaceVisit, PlaceWalk, SlotStore, SlotVisit},
};
use crate::{
    call::{LendOne, OutboxOf, PartsOf, RootOf, SlotCall, SlotDesc, SlotYield},
    hooks::{EndCause, EndReasonOf},
    request::LinkedToError,
    structure::{
        ChildAddresses, End, FromAddress, NodeKind, ParentPath, PathFrom, RemainsOf, SplitLast,
    },
};

struct EndByAt<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    by: place::EndBy,
    outbox: &'r OutboxOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for EndByAt<'_, Q> {
    type Out = ();
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) {
        Pos::end_by(self.root, self.at, self.by, self.outbox)
    }
}
struct DetachAt<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for DetachAt<'_, Q> {
    type Out = bool;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> bool {
        Pos::detach(self.root, self.at)
    }
}
struct Stop {
    node: usize,
}
impl<Q: DriverComposition> SlotVisit<Q> for Stop {
    type Out = ();
    fn visit<S, Pos>(self, store: &mut SlotStore<Pos::Running, S::Yield, AddressOf<Q>>)
    where
        S: SlotDesc<At: FromAddress<RootOf<Q>> + 'static> + SlotYield<Q>,
        Pos: SlotCall<Q, S> + for<'a> LendOne<'a, Q, S>,
    {
        if let Some(call) = store.calls.get_mut(self.node) {
            *call = None;
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
    /// Remove the node at `at`, for `reason`, with every node below it
    /// (each told that a node above ended), as a request's edit handle
    /// does. The path is at least one step long: the root is ended with
    /// [`end`](Driver::end). Refused when no node is at the path (always,
    /// once the driver ended), or when nodes link to it: a node others link to is removed after the last
    /// of them, not on its own. A path is by handles, as
    /// [`add`](Driver::add) says: one kept after its node ended may reach
    /// a node added later.
    ///
    /// The root removed by a path: a path to remove is at least one step long,
    /// and the root is ended with [`Driver::end`]:
    ///
    /// ```compile_fail
    /// # #![feature(impl_trait_in_assoc_type)]
    /// # use core::time::Duration;
    /// # use frunk_core::{HList, hlist::HNil};
    /// # use nitori_driver::{
    /// #     call::ShareCalls,
    /// #     composition,
    /// #     driver::Driver,
    /// #     hooks::Part,
    /// #     structure::{AtMostOne, Child, End, NodeKind, Share},
    /// # };
    /// # #[derive(Clone, Default)]
    /// # struct Clock;
    /// # impl nitori_clock::MonotonicClock for Clock {
    /// #     type Instant = Duration;
    /// #     type Sleep = core::future::Pending<()>;
    /// #     fn now(&self) -> Duration { Duration::ZERO }
    /// #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
    /// #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
    /// #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
    /// # }
    /// # struct Root;
    /// # struct Layer;
    /// # impl NodeKind for Root {
    /// #     type Children = HList![Child<Layer, AtMostOne>];
    /// #     type Linked = HNil;
    /// # }
    /// # impl NodeKind for Layer {
    /// #     type Children = HNil;
    /// #     type Linked = HNil;
    /// # }
    /// # struct Base;
    /// # impl<L> Part<L> for Base { type Args = (); }
    /// # impl ShareCalls<Root> for Base { type Calls = HNil; }
    /// # impl ShareCalls<Layer> for Base { type Calls = HNil; }
    /// # impl Share<Root> for Base { type Data = HNil; }
    /// # impl Share<Layer> for Base { type Data = HNil; }
    /// # struct Q;
    /// # composition!(impl driver for Q: Root, HList![Base], Clock);
    /// let mut driver = Driver::<Q>::new(Clock, frunk_core::hlist![()], ()).unwrap();
    /// let _ = driver.remove(End, ());
    /// ```
    pub fn remove<X, N>(
        &mut self,
        at: X,
        reason: EndReasonOf<TargetOf<Q, X>>,
    ) -> Result<(), RemoveError>
    where
        X: PathFrom<RootOf<Q>> + PlaceKey<RootOf<Q>> + ParentPath + SplitLast + 'static,
        PlacesOf<Q>: FindPlace<X, N> + NthPlace<RootOf<Q>, N, Item = Place<X>>,
        Q::Places: PlaceNth<Q, PlacesOf<Q>, N>,
    {
        if self.end_if_panicked() {
            return Err(RemoveError::NoNode);
        }
        let id = self.find(&at.address()).ok_or(RemoveError::NoNode)?;
        debug_assert_ne!(
            id,
            self.root_id(),
            "a path of a step or more is not the root's"
        );
        if self.linked_to(id) {
            return Err(RemoveError::LinkedTo {
                source: LinkedToError,
            });
        }
        self.running = true;
        self.outbox.set_now(self.clock.now());
        let mut todo = Vec::new();
        self.end_typed::<Place<X>, PlacePositionsAt<Q, N>>(id, reason, &mut todo);
        self.settle(todo);
        self.running = false;
        Ok(())
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
    /// End the root, for `reason`, with every node below it: the nodes
    /// below end first; the root's components write what their parts keep
    /// ([`remains`](Driver::remains)) and end. The driver's future is
    /// ready afterwards.
    ///
    /// The events of the ending are not handled: the driver ends with the
    /// root, so a [`NodeEnding`](crate::event::NodeEnding) of a node below
    /// does not reach the root's components, as it does when a node is
    /// removed with [`remove`](Driver::remove).
    pub fn end(&mut self, reason: EndReasonOf<RootOf<Q>>) {
        if self.end_if_panicked() {
            return;
        }
        self.running = true;
        self.outbox.set_now(self.clock.now());
        let id = self.root_id();
        let mut todo = Vec::new();
        self.end_typed::<Place<End>, PlacePositionsAt<Q, I0>>(id, reason, &mut todo);
        self.running = false;
    }
    /// End the node `id` of the place `Pl`, with the nodes below it, for
    /// `reason`; the root ends the driver.
    pub(super) fn end_typed<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(
        &mut self,
        id: usize,
        reason: EndReasonOf<TargetOf<Q, Pl::At>>,
        todo: &mut Vec<Hit>,
    ) {
        if self.live(id).is_none() {
            return;
        }
        self.remove_subtree(
            id,
            |root, at, outbox| Pos::end(root, at, EndCause::Reason(&reason), outbox),
            place::EndBy::Above,
            todo,
        );
    }
    /// Every node below and at `id`, in the order they end: a node after
    /// the nodes below it, a child of a kind others link to after its
    /// siblings' subtrees.
    fn removal_order(&self, id: usize, out: &mut Vec<usize>) {
        if let Some(n) = self.live(id) {
            let (targets, others): (Vec<usize>, Vec<usize>) = n.children.iter().partition(|&&c| {
                self.live(c)
                    .is_some_and(|c| self.places[c.place].link_target)
            });
            for c in others.into_iter().chain(targets) {
                self.removal_order(c, out);
            }
            out.push(id);
        }
    }

    /// End the node `id` with the nodes below it; `own_end` runs the end
    /// hooks of `id` itself. What it reaches goes into `todo`; ending the
    /// root ends the driver. A node that no node links to any more once
    /// they ended ends after them, told it was unlinked.
    pub(super) fn remove_subtree(
        &mut self,
        id: usize,
        own_end: impl FnOnce(&mut RootNode<Q>, &AddressOf<Q>, &OutboxOf<Q>),
        others: place::EndBy,
        todo: &mut Vec<Hit>,
    ) {
        let mut order = Vec::new();
        self.removal_order(id, &mut order);
        let is_root = id == self.root_id();
        let mut own_end = Some(own_end);
        let mut unlinked = Vec::new();
        for n in order {
            let Some(entry) = self.live(n) else { continue };
            let (p, addr) = (entry.place, entry.addr);
            unlinked.extend(entry.link);
            let ending = self.places[p].info.ending;
            self.deliver(n, ending, None, true, todo);
            for c in 0..self.places[p].info.comps.len() {
                self.set_deadline(n, c, None);
            }
            for k in 0..self.places[p].calls.len() {
                let s = self.places[p].calls[k];
                let _ = <Cursor<I0> as CallWalk<Q>>::visit(s, &mut self.calls, Stop { node: n });
            }
            let Some(root) = self.root.as_mut() else {
                return;
            };
            if n == id {
                if is_root {
                    let mut remains = Default::default();
                    <PlacePositionsAt<Q, I0> as PlaceOps<Q, Place<End>>>::remains(
                        root,
                        &addr,
                        &mut remains,
                        &self.outbox,
                    );
                    self.remains = Some(remains);
                }
                if let Some(end) = own_end.take() {
                    end(root, &addr, &self.outbox);
                }
            } else {
                let _ = <Cursor<I0> as PlaceWalk<Q>>::visit(
                    p,
                    EndByAt::<Q> {
                        root,
                        at: &addr,
                        by: others,
                        outbox: &self.outbox,
                    },
                );
            }
            let writes = self.comp_writes(p);
            self.effects(n, &writes, None, todo);
            if !is_root || n != id {
                let root = self.root.as_mut().expect("the root is there while it ends");
                let _ = <Cursor<I0> as PlaceWalk<Q>>::visit(p, DetachAt::<Q> { root, at: &addr });
            }
            self.retire(n);
        }
        if is_root {
            self.finish();
            return;
        }
        self.end_unlinked(unlinked, todo);
    }
    /// End each of `targets` that is there and no node links to any more:
    /// it is told it was unlinked, the nodes below it that one above
    /// ended.
    fn end_unlinked(&mut self, targets: Vec<usize>, todo: &mut Vec<Hit>) {
        for target in targets {
            let Some(p) = self.live(target).map(|n| n.place) else {
                continue;
            };
            if self.linked_to(target) {
                continue;
            }
            self.remove_subtree(
                target,
                |root, at, outbox| {
                    let _ = <Cursor<I0> as PlaceWalk<Q>>::visit(
                        p,
                        EndByAt::<Q> {
                            root,
                            at,
                            by: place::EndBy::Unlinked,
                            outbox,
                        },
                    );
                },
                place::EndBy::Above,
                todo,
            );
        }
    }
    fn retire(&mut self, id: usize) {
        let slot = core::mem::replace(&mut self.nodes[id], NodeSlot::Free);
        if let NodeSlot::Live(n) = slot {
            n.head.dead.store(true, SeqCst);
            if let Some(parent) = n.parent
                && let NodeSlot::Live(p) = &mut self.nodes[parent]
            {
                p.children.retain(|&c| c != id);
            }
            self.nodes[id] = NodeSlot::Retired(n.head);
        }
    }
    /// The root ended: the driver does nothing more.
    pub(super) fn finish(&mut self) {
        self.ended = true;
        self.root = None;
        self.shared.closed.store(true, SeqCst);
        let _ = self.outbox.take();
        self.timers.clear();
        self.deadlines.clear();
        self.sleep = None;
        self.pending.clear();
        self.deferred.clear();
        self.deferred_timers.clear();
        if let Some(outer) = &self.outer {
            outer.wake_by_ref();
        }
    }
    fn comp_writes(&self, p: usize) -> Vec<TypeId> {
        let mut out = Vec::new();
        for c in &self.places[p].info.comps {
            out.extend_from_slice(&c.writes);
        }
        out
    }
}

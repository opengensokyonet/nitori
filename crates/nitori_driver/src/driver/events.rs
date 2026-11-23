//! What a lend leaves: the requests it made and the events it published,
//! handled round after round, and the delivery of an event to the
//! components that hear it.
use alloc::vec::Vec;
use core::any::TypeId;

use frunk_core::indices::Here as I0;

use super::{
    AddressOf, Driver, DriverComposition, Hit, NthPlace, Place, PlaceDesc, PlaceNth, PlaceOps,
    PlacesOf, RequestsOf, RootNode, Target, edit,
    walk::{CallWalk, Cursor, PlaceVisit, PlaceWalk},
};
use crate::{
    call::{OutboxOf, PartsOf, RootOf},
    event::{Scope, Triggered},
    structure::{ChildAddresses, End, NodeKind, RemainsOf},
};

struct OnEvents<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    c: usize,
    events: Triggered<'r>,
    outbox: &'r OutboxOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for OnEvents<'_, Q> {
    type Out = Option<RequestsOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::on_events(self.root, self.at, self.c, self.events, self.outbox)
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
    /// Have the node a request is for handle it; what its edits reach goes
    /// into `todo`.
    pub(super) fn handle(&mut self, request: RequestsOf<Q>, todo: &mut Vec<Hit>) {
        if self.ended {
            return;
        }
        <RequestsOf<Q> as edit::Deliver<Q, I0>>::deliver(request, self);
        todo.append(&mut self.pending);
    }
    /// Handle the hits, round after round, until a round publishes nothing
    /// heard: a call is woken; a component's handler runs with the events
    /// that reached it, and its deadline is read again. A request a handler
    /// returns is handled at once, and what it reaches goes into the next
    /// round.
    pub(super) fn settle(&mut self, mut todo: Vec<Hit>) {
        let mut round = 0usize;
        while !todo.is_empty() && !self.ended {
            round += 1;
            let bound = self
                .places
                .iter()
                .map(|p| p.positions() + p.info.comps.len())
                .sum::<usize>()
                * (self.nodes.len() + 1)
                + 2;
            if round > bound {
                // a component publishes or moves deadlines without end; the
                // rest waits for the next poll
                debug_assert!(round <= bound, "events did not settle");
                self.deferred.append(&mut todo);
                if let Some(outer) = &self.outer {
                    outer.wake_by_ref();
                }
                break;
            }
            todo.sort_unstable_by_key(|h| (h.node, h.serial, h.target));
            let mut merged: Vec<Hit> = Vec::with_capacity(todo.len());
            for hit in todo {
                match merged.last_mut() {
                    Some(last)
                        if (last.node, last.serial, last.target)
                            == (hit.node, hit.serial, hit.target) =>
                    {
                        last.events |= hit.events
                    }
                    _ => merged.push(hit),
                }
            }
            let mut next = Vec::new();
            for Hit {
                node,
                serial,
                target,
                events,
            } in merged
            {
                if self.ended {
                    break;
                }
                let Some(n) = self.live(node).filter(|n| n.serial == serial) else {
                    continue;
                };
                let (p, addr) = (n.place, n.addr);
                match target {
                    Target::Call(k) => self.mark(node, 1 << k),
                    Target::Comp(c) => {
                        if events != 0 {
                            let Some(root) = self.root.as_mut() else {
                                break;
                            };
                            let request = <Cursor<I0> as PlaceWalk<Q>>::visit(
                                p,
                                OnEvents::<Q> {
                                    root,
                                    at: &addr,
                                    c,
                                    events: Triggered::new(events, &self.places[p].handled[c]),
                                    outbox: &self.outbox,
                                },
                            )
                            .flatten();
                            let writes = self.places[p].info.comps[c].writes.clone();
                            self.effects(node, &writes, None, &mut next);
                            if let Some(request) = request {
                                self.handle(request, &mut next);
                            }
                        }
                        if self.places[p].info.comps[c].timed
                            && self.live(node).is_some_and(|n| n.serial == serial)
                        {
                            self.read_deadline(node, c);
                        }
                    }
                }
            }
            todo = next;
        }
    }
    /// After a lend at `id`: its effects and the requests it made,
    /// settled.
    pub(super) fn after_lend(
        &mut self,
        id: usize,
        writes: &[TypeId],
        skip: Option<(usize, Target)>,
        requests: impl IntoIterator<Item = RequestsOf<Q>>,
    ) {
        let mut todo = Vec::new();
        self.effects(id, writes, skip, &mut todo);
        for request in requests {
            self.handle(request, &mut todo);
        }
        self.settle(todo);
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
    /// Run `f` with the hits of the request being handled as its `todo`.
    pub(super) fn editing<T>(&mut self, f: impl FnOnce(&mut Self, &mut Vec<Hit>) -> T) -> T {
        let mut todo = core::mem::take(&mut self.pending);
        let out = f(self, &mut todo);
        self.pending.append(&mut todo);
        out
    }
    fn hit(&self, node: usize, target: Target, events: u64) -> Hit {
        Hit {
            node,
            serial: self.live(node).map_or(0, |n| n.serial),
            target,
            events,
        }
    }
    /// After a lend at `id` that took the components `writes` exclusive:
    /// the deadlines of those types to read again, and the events it
    /// published, as hits. `skip` is a call's own position.
    pub(super) fn effects(
        &mut self,
        id: usize,
        writes: &[TypeId],
        skip: Option<(usize, Target)>,
        out: &mut Vec<Hit>,
    ) {
        for (depth, event) in self.outbox.take() {
            if let Some(at) = self.ancestor_at(id, depth) {
                self.deliver(at, event, skip, false, out);
            }
        }
        if writes.is_empty() {
            return;
        }
        for m in self.live_ids().collect::<Vec<_>>() {
            let p = self.nodes[m].live().map_or(0, |n| n.place);
            for (c, comp) in self.places[p].info.comps.iter().enumerate() {
                if comp.timed && writes.contains(&comp.type_id) {
                    out.push(self.hit(m, Target::Comp(c), 0));
                }
            }
        }
    }
    /// The hits of the event `event` published at `at`: every subscriber
    /// whose scope has it, `skip` aside; `above` for an event that reaches
    /// only the nodes above.
    pub(super) fn deliver(
        &self,
        at: usize,
        event: TypeId,
        skip: Option<(usize, Target)>,
        above: bool,
        out: &mut Vec<Hit>,
    ) {
        let start = self.subscribers.partition_point(|&(e, _)| e < event);
        let Some(at_place) = self.live(at).map(|n| n.place) else {
            return;
        };
        for &(e, sub) in &self.subscribers[start..] {
            if e != event {
                break;
            }
            for m in self.live_ids() {
                if self.live(m).map(|n| n.place) != Some(sub.place) {
                    continue;
                }
                if skip == Some((m, sub.target)) {
                    continue;
                }
                let heard = match sub.scope {
                    Scope::Around => {
                        if above {
                            self.is_above(m, at)
                        } else {
                            m == at || self.is_above(m, at) || self.is_above(at, m)
                        }
                    }
                    Scope::At { depth, place } => {
                        self.places[at_place].info.pre == place
                            && self.ancestor_at(at, depth).is_some()
                            && self.ancestor_at(at, depth) == self.ancestor_at(m, depth)
                            && (!above || self.is_above(m, at))
                    }
                };
                if heard {
                    out.push(self.hit(m, sub.target, sub.bit.map_or(0, |b| 1 << b)));
                }
            }
        }
    }
}

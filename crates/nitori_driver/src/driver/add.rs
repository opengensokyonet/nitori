//! Adding a node: from outside ([`Driver::add`]) and as a node starts.
use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use core::any::TypeId;

use frunk_core::indices::Here as I0;
use nitori_clock::MonotonicClock;

use super::{
    AddError, AddressOf, Driver, DriverComposition, FindPlace, Hit, NodeEntry, NodeSlot, NthPlace,
    Place, PlaceAt, PlaceDesc, PlaceKey, PlaceKeyOf, PlaceNth, PlaceOps, PlacePositionsAt,
    PlacesOf, RequestsOf, TargetOf, edit,
    wake::NodeHead,
    walk::{CallWalk, Cursor, Number, PlaceWalk, SlotStore, SlotVisit},
};
use crate::{
    call::{LendOne, PartsOf, RootOf, SlotCall, SlotDesc, SlotYield},
    hooks::StartOf,
    structure::{
        Address, Append, ChildAddresses, ChildAt, ChildOfKind, End, FromAddress, HandleAt, KeyAt,
        KindFor, NodeKind, NodePath, ParentChildren, ParentPath, PathFrom, RemainsOf,
    },
};

struct Start<'r, Q: DriverComposition> {
    node: usize,
    at: &'r AddressOf<Q>,
}
impl<Q: DriverComposition> SlotVisit<Q> for Start<'_, Q> {
    type Out = ();
    fn visit<S, Pos>(self, store: &mut SlotStore<Pos::Running, S::Yield, AddressOf<Q>>)
    where
        S: SlotDesc<At: FromAddress<RootOf<Q>> + 'static> + SlotYield<Q>,
        Pos: SlotCall<Q, S> + for<'a> LendOne<'a, Q, S>,
    {
        let Some(at) = S::At::from_address(self.at) else {
            return;
        };
        if store.calls.len() <= self.node {
            store.calls.resize_with(self.node + 1, || None);
        }
        store.calls[self.node] = Some(Box::pin(Pos::start(&at)));
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
    /// Add a node of the kind `CK` under the node at `parent`, under
    /// `key`, starting from `start`, as a request's edit handle does: it is
    /// built from the view of its parent, its calls start woken, its
    /// deadlines are read and [`NodeStarted`](crate::event::NodeStarted) is
    /// published at it. Returns its path. The position `I` among the
    /// parent's children and the number `N` of the new node's place are
    /// inferred.
    ///
    /// The path returned is by handles: after the node ends, a store that
    /// reuses its slots ([`SlotVec`](crate::structure::SlotVec)) may give
    /// the same handle to a node added later, which the same path then
    /// reaches. A request names its node by keys and does not have this.
    /// The node may end before `add` returns: the events its start
    /// publishes are handled first, and a request a handler returns may
    /// end it. It was added all the same; the path is the one it had, and
    /// may reach no node or one added after it.
    #[allow(clippy::type_complexity)]
    pub fn add<CK, X, I, N>(
        &mut self,
        parent: X,
        key: KeyAt<ParentChildren<RootOf<Q>, X>, I>,
        start: StartOf<CK>,
    ) -> Result<<X as Append<I, HandleAt<ParentChildren<RootOf<Q>, X>, I>>>::Out, AddError>
    where
        CK: KindFor<PartsOf<Q>>,
        X: NodePath<RootOf<Q>, PartsOf<Q>> + Append<I, HandleAt<ParentChildren<RootOf<Q>, X>, I>>,
        ParentChildren<RootOf<Q>, X>: ChildAt<I, PartsOf<Q>> + ChildOfKind<CK, I>,
        <X as Append<I, HandleAt<ParentChildren<RootOf<Q>, X>, I>>>::Out: FromAddress<RootOf<Q>>
            + PlaceKey<RootOf<Q>, Key = KeyAt<ParentChildren<RootOf<Q>, X>, I>>
            + PathFrom<RootOf<Q>, Target = CK>
            + ParentPath
            + 'static,
        PlacesOf<Q>: FindPlace<<X as Append<I, HandleAt<ParentChildren<RootOf<Q>, X>, I>>>::Out, N>
            + NthPlace<
                RootOf<Q>,
                N,
                Item = Place<<X as Append<I, HandleAt<ParentChildren<RootOf<Q>, X>, I>>>::Out>,
            >,
        Q::Places: PlaceNth<Q, PlacesOf<Q>, N>,
        N: Number,
    {
        if self.ended {
            return Err(AddError::Ended);
        }
        let Some(parent_id) = self.find(&parent.address()) else {
            return Err(AddError::no_parent());
        };
        self.outbox.set_now(self.clock.now());
        let mut todo = Vec::new();
        let added = self.add_typed::<PlaceAt<Q, N>, PlacePositionsAt<Q, N>>(
            N::VALUE,
            parent_id,
            key,
            start,
            &mut todo,
        );
        // the path is read before the events are handled: a handler may
        // end the node, and its number may go to another node
        let path = added.map(|id| {
            // Invariant: `add_typed` registered the node just now.
            let addr = self.live(id).expect("the node just added is there").addr;
            // Invariant: the node was added under `parent` at the position
            // `I`, so its address is one of the path type `parent` and `I`
            // give.
            FromAddress::from_address(&addr).expect("the address of a node added at the path")
        });
        // the linked node a link built was built through the edit
        // operations, which keep their hits aside
        todo.append(&mut self.pending);
        self.settle(todo);
        path
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
    pub(super) fn register(
        &mut self,
        addr: AddressOf<Q>,
        place: usize,
        parent: Option<usize>,
    ) -> usize {
        let id = self
            .nodes
            .iter()
            .position(|n| match n {
                NodeSlot::Free => true,
                NodeSlot::Live(_) => false,
                NodeSlot::Retired(h) => NodeHead::reusable(h),
            })
            .unwrap_or_else(|| {
                self.nodes.push(NodeSlot::Free);
                self.nodes.len() - 1
            });
        let positions = self.places[place].positions();
        self.serial += 1;
        self.nodes[id] = NodeSlot::Live(NodeEntry {
            head: Arc::new(NodeHead::new(id)),
            serial: self.serial,
            place,
            addr,
            parent,
            children: Vec::new(),
            link: None,
            wakers: vec![None; positions],
        });
        if let Some(parent) = parent
            && let NodeSlot::Live(p) = &mut self.nodes[parent]
        {
            p.children.push(id);
        }
        id
    }
    /// A new node: its calls start, its calls and its actions before the
    /// calls are woken, its deadlines are read, and that it started is
    /// published.
    pub(super) fn started(&mut self, id: usize, todo: &mut Vec<Hit>) {
        let p = self.nodes[id].live().map(|n| n.place).unwrap_or(0);
        let addr = self.nodes[id]
            .live()
            .map(|n| n.addr)
            .unwrap_or_else(Address::here);
        for k in 0..self.places[p].calls.len() {
            let s = self.places[p].calls[k];
            let _ = <Cursor<I0> as CallWalk<Q>>::visit(
                s,
                &mut self.calls,
                Start::<Q> {
                    node: id,
                    at: &addr,
                },
            );
        }
        self.woken(id, todo);
    }
    /// A node started or moved: its calls and its actions before the calls
    /// are woken, its deadlines are read, and that it started is
    /// published.
    pub(super) fn woken(&mut self, id: usize, todo: &mut Vec<Hit>) {
        let p = self.nodes[id].live().map(|n| n.place).unwrap_or(0);
        let woken = self.places[p].calls_mask() | self.places[p].before_mask();
        self.mark(id, woken);
        for c in 0..self.places[p].info.comps.len() {
            if self.places[p].info.comps[c].timed {
                self.read_deadline(id, c);
            }
        }
        let started = self.places[p].info.started;
        self.deliver(id, started, None, false, todo);
    }

    /// Build a node of the place `Pl` (whose number is `p` and positions
    /// `Pos`) under the node `parent`, under `key`, from `start`; refused
    /// when the key is taken, or when `parent` or the root is not there. A
    /// node whose kind links to another has it built first, under its own
    /// key, when it is not there.
    pub(super) fn add_typed<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(
        &mut self,
        p: usize,
        parent: usize,
        key: PlaceKeyOf<Q, Pl::At>,
        mut start: StartOf<TargetOf<Q, Pl::At>>,
        todo: &mut Vec<Hit>,
    ) -> Result<usize, AddError> {
        let parent_addr = self.live(parent).ok_or(AddError::no_parent())?.addr;
        debug_assert_eq!(
            self.places[p].info.parent,
            self.live(parent).map(|n| self.places[n.place].info.path),
            "a place's nodes are added under nodes of its parent place",
        );
        let root = self.root.as_ref().ok_or(AddError::Ended)?;
        if Pos::find_child(root, &parent_addr, &key).is_some() {
            return Err(AddError::occupied());
        }
        let link = <Pos::Link as edit::LinkAdd<_, _, _>>::ensure(self, &key, &mut start);
        let root = self.root.as_mut().ok_or(AddError::Ended)?;
        let addr = Pos::add(root, &parent_addr, key, start, &self.args, &self.outbox);
        // what the builds published, at the parent and above, and the
        // deadlines of what they took exclusive there
        let writes: Vec<TypeId> = self.places[p]
            .info
            .comps
            .iter()
            .flat_map(|c| c.build_writes.iter().copied())
            .collect();
        self.effects(parent, &writes, None, todo);
        let id = self.register(addr, p, Some(parent));
        if let NodeSlot::Live(n) = &mut self.nodes[id] {
            n.link = link;
        }
        self.started(id, todo);
        Ok(id)
    }
}

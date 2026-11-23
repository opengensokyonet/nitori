//! Finding nodes: by number, by address, by key, and the nodes above and
//! below one.
use alloc::vec::Vec;

use frunk_core::indices::Here as I0;

use super::{
    AddressOf, Driver, DriverComposition, NodeEntry, NodeSlot, NthPlace, Place, PlaceDesc,
    PlaceKeyOf, PlaceNth, PlaceOps, PlacesOf,
    walk::{CallWalk, Cursor, PlaceWalk},
};
use crate::{
    call::{PartsOf, RootOf},
    structure::{ChildAddresses, End, KeyAddress, NodeKind, RemainsOf, resolve_node},
};

impl<Q: DriverComposition> Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
    pub(super) fn root_id(&self) -> usize {
        // Invariant: the root is registered first and keeps its number
        // until it ends.
        0
    }
    pub(super) fn live(&self, id: usize) -> Option<&NodeEntry<AddressOf<Q>>> {
        self.nodes.get(id).and_then(NodeSlot::live)
    }
    /// The node `id`, while it is the one registered as `serial`: its
    /// number goes to another node once it ends.
    pub(super) fn live_as(&self, id: usize, serial: u64) -> Option<&NodeEntry<AddressOf<Q>>> {
        self.live(id).filter(|n| n.serial == serial)
    }
    pub(super) fn find(&self, addr: &AddressOf<Q>) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.live().is_some_and(|n| n.addr == *addr))
    }
    pub(super) fn live_ids(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.nodes.len()).filter(|&id| self.live(id).is_some())
    }
    pub(super) fn depth(&self, id: usize) -> usize {
        self.live(id).map_or(0, |n| self.places[n.place].info.depth)
    }
    /// The node at `depth` on the way up from `id`, `id` included.
    pub(super) fn ancestor_at(&self, id: usize, depth: usize) -> Option<usize> {
        let mut at = id;
        loop {
            let n = self.live(at)?;
            let d = self.places[n.place].info.depth;
            if d == depth {
                return Some(at);
            }
            if d < depth {
                return None;
            }
            at = n.parent?;
        }
    }
    /// Whether `above` is strictly above `id`.
    pub(super) fn is_above(&self, above: usize, id: usize) -> bool {
        let mut at = self.live(id).and_then(|n| n.parent);
        while let Some(a) = at {
            if a == above {
                return true;
            }
            at = self.live(a).and_then(|n| n.parent);
        }
        false
    }
    /// Whether nodes link to the node `id`.
    pub(super) fn linked_to(&self, id: usize) -> bool {
        self.nodes
            .iter()
            .any(|n| n.live().is_some_and(|n| n.link == Some(id)))
    }
    /// The node of the place `Pl` under the node `parent`, kept under
    /// `key`.
    pub(super) fn find_typed<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(
        &self,
        parent: usize,
        key: &PlaceKeyOf<Q, Pl::At>,
    ) -> Option<usize> {
        let parent_addr = self.live(parent)?.addr;
        let addr = Pos::find_child(self.root.as_ref()?, &parent_addr, key)?;
        self.find(&addr)
    }
    /// The node at the key address `at`.
    pub(super) fn find_key<T>(&self, at: &KeyAddress<RootOf<Q>, T>) -> Option<usize> {
        let addr = resolve_node(self.root.as_ref()?, at.route())?;
        self.find(&addr)
    }
    /// Every node in the subtree of `id`, `id` first, each node before the
    /// nodes below it.
    pub(super) fn subtree(&self, id: usize, out: &mut Vec<usize>) {
        if let Some(n) = self.live(id) {
            out.push(id);
            for &c in &n.children {
                self.subtree(c, out);
            }
        }
    }
}

//! Moving a node, with the nodes below it, under another parent.
use alloc::vec::Vec;

use frunk_core::indices::Here as I0;

use super::{
    AddressOf, Driver, DriverComposition, Hit, NodeSlot, NthPlace, Place, PlaceDesc, PlaceNth,
    PlaceOps, PlacesOf, RootNode,
    walk::{CallWalk, Cursor, PlaceVisit, PlaceWalk},
};
use crate::{
    call::{PartsOf, RootOf},
    structure::{ChildAddresses, End, MoveError, NodeKind, OccupiedError, RemainsOf},
};

struct MoveUnder<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    to: &'r AddressOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for MoveUnder<'_, Q> {
    type Out = Result<AddressOf<Q>, MoveError>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::move_under(self.root, self.at, self.to)
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
    /// Move the node `id` under the node `to`, of the place of its parent:
    /// it ends where it was for the nodes above, its subtree's addresses
    /// follow it, and it starts where it goes; its calls go on. Both are
    /// there; refused when `to` has a child of its kind under its key.
    pub(super) fn move_node(
        &mut self,
        id: usize,
        to: usize,
        todo: &mut Vec<Hit>,
    ) -> Result<(), OccupiedError> {
        // Invariant: the edit handle found both a moment ago, and nothing
        // was handled since.
        let n = self.live(id).expect("the node to move was found");
        let (p, at, from) = (n.place, n.addr, n.parent);
        let to_addr = self.live(to).expect("the new parent was found").addr;
        if from == Some(to) {
            return Ok(());
        }
        let root = self
            .root
            .as_mut()
            .expect("the root is there while a request is handled");
        let moved = <Cursor<I0> as PlaceWalk<Q>>::visit(
            p,
            MoveUnder::<Q> {
                root,
                at: &at,
                to: &to_addr,
            },
        )
        // Invariant: `p` is the place of a node, so the walk reaches it.
        .expect("the walk reaches the place of a node");
        let new = match moved {
            Ok(new) => new,
            Err(MoveError::Occupied { source }) => return Err(source),
            // Invariant: both nodes are in the tree, at the paths of their
            // addresses.
            Err(e) => unreachable!("both ends of a move are there: {e}"),
        };
        // it ends where it was: the nodes above it there hear it
        let ending = self.places[p].info.ending;
        self.deliver(id, ending, None, true, todo);
        let levels = self.places[p].info.depth;
        let mut moved = Vec::new();
        self.subtree(id, &mut moved);
        for &m in &moved {
            if let NodeSlot::Live(e) = &mut self.nodes[m] {
                // Invariant: every node of the subtree has the moved node's
                // address as its first levels.
                e.addr = e
                    .addr
                    .splice(&new, levels)
                    .expect("a node below has the address of the node above it first");
            }
        }
        if let Some(from) = from
            && let NodeSlot::Live(f) = &mut self.nodes[from]
        {
            f.children.retain(|&c| c != id);
        }
        if let NodeSlot::Live(t) = &mut self.nodes[to] {
            t.children.push(id);
        }
        if let NodeSlot::Live(e) = &mut self.nodes[id] {
            e.parent = Some(to);
        }
        for &m in &moved[1..] {
            let q = self.live(m).map_or(0, |n| n.place);
            for c in 0..self.places[q].info.comps.len() {
                if self.places[q].info.comps[c].timed {
                    self.read_deadline(m, c);
                }
            }
        }
        self.woken(id, todo);
        Ok(())
    }
}

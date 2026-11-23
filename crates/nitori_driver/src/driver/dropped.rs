//! A driver dropped before the root ended, or after a panic through a
//! poll or another entry ([`Driver::new`], [`add`](Driver::add),
//! [`remove`](Driver::remove), [`end`](Driver::end)).
use alloc::{vec, vec::Vec};

use frunk_core::indices::Here as I0;

use super::{
    AddressOf, Driver, DriverComposition, NthPlace, Place, PlaceDesc, PlaceNth, PlaceOps,
    PlacePositionsAt, PlacesOf, RootNode, place,
    walk::{CallWalk, Cursor, PlaceVisit, PlaceWalk},
};
use crate::{
    call::{OutboxOf, PartsOf, RootOf},
    hooks::AbortCause,
    structure::{Address, ChildAddresses, End, NodeKind, RemainsOf},
};

struct Aborted<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    cause: AbortCause,
    outbox: &'r OutboxOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for Aborted<'_, Q> {
    type Out = ();
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) {
        Pos::aborted(self.root, self.at, self.cause, self.outbox)
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
    /// If a panic went through an entry (a poll, `new`, `add`, `remove`,
    /// `end`) and the caller caught it, end the driver as after a panic:
    /// the root's components are told and write their remains. Then
    /// whether the driver ended, now or before.
    pub(super) fn end_if_panicked(&mut self) -> bool {
        if self.running {
            self.end_dropped();
        }
        self.ended
    }
    /// Dropped before the root ended: every component is told, the nodes
    /// above first, and every node ends. After a panic through an entry,
    /// only the root's components are told and write their remains, and
    /// no node ends: the tree may be half changed.
    pub(super) fn end_dropped(&mut self) {
        if self.ended {
            return;
        }
        let panicked = self.running;
        let cause = if panicked {
            AbortCause::Panicked
        } else {
            AbortCause::Dropped
        };
        let _ = self.outbox.take();
        let mut told: Vec<usize> = if panicked {
            vec![self.root_id()]
        } else {
            self.live_ids().collect()
        };
        told.sort_by_key(|&id| self.depth(id));
        for id in told {
            let Some(n) = self.live(id) else { continue };
            let (p, addr) = (n.place, n.addr);
            let Some(root) = self.root.as_mut() else {
                return;
            };
            let _ = <Cursor<I0> as PlaceWalk<Q>>::visit(
                p,
                Aborted::<Q> {
                    root,
                    at: &addr,
                    cause,
                    outbox: &self.outbox,
                },
            );
        }
        // nothing written or published is handled
        let _ = self.outbox.take();
        if panicked {
            if let Some(root) = self.root.as_mut() {
                let mut remains = Default::default();
                <PlacePositionsAt<Q, I0> as PlaceOps<Q, Place<End>>>::remains(
                    root,
                    &Address::here(),
                    &mut remains,
                    &self.outbox,
                );
                self.remains = Some(remains);
            }
            self.running = false;
            self.finish();
        } else {
            let id = self.root_id();
            self.remove_subtree(
                id,
                |root, at, outbox| {
                    <PlacePositionsAt<Q, I0> as PlaceOps<Q, Place<End>>>::end_by(
                        root,
                        at,
                        place::EndBy::Dropped,
                        outbox,
                    )
                },
                place::EndBy::Dropped,
                &mut Vec::new(),
            );
        }
    }
}

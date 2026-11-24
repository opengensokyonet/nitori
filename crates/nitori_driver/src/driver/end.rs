//! Ending a subtree: the nodes below before the node above, each with
//! `NodeEnding` fired, its calls dropped, its components ended and taken
//! out of the tree; and the driver dropped.
use alloc::vec::Vec;

use frunk_core::hlist::Selector;

use super::{
    Core,
    visit::{DropCalls, EndNode, Ending},
};
use crate::{
    call::{
        __pack::{CompList, KindDesc, KindOps, PackIndex, PackList, RequestQueues},
        Composition, KindsOf,
        pack_at::{IxDesc, IxEndReason, IxOps, ix_table},
        request_lists, take_wakes,
    },
    component::EndCause,
    structure::NodeKind,
};

impl<Q: Composition> Core<Q> {
    /// The nodes at and below `top`, each after the nodes below it.
    fn post_order(&self, top: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = alloc::vec![(top, false)];
        while let Some((n, seen)) = stack.pop() {
            if seen {
                out.push(n);
                continue;
            }
            stack.push((n, true));
            if let Some(node) = self.nodes.live(n) {
                // The children in reverse, so that they end in the order
                // they were added.
                stack.extend(node.children.iter().rev().map(|&c| (c, false)));
            }
        }
        out
    }

    /// The node `number` ended: its number retired, its items off the
    /// queue, and it gone from its parent's children.
    fn forget(&mut self, number: u32) {
        self.ready.remove_node(number);
        self.settles.retain(|(item, _)| item.node != number);
        let Some(node) = self.nodes.retire(number) else {
            return;
        };
        match node.parent {
            Some(parent) => {
                if let Some(parent) = self.nodes.live_mut(parent) {
                    parent.children.retain(|&c| c != number);
                }
            }
            None => self.root_ended = true,
        }
    }

    /// End the node `number`, the nodes below it ended already, for
    /// `ending`.
    fn end_by_kind(&mut self, number: u32, ending: Ending) {
        let Some(kind) = self.nodes.live(number).map(|n| n.kind) else {
            return;
        };
        let Self {
            root, cx, tables, ..
        } = self;
        <Q::Pack as PackList<Q, KindsOf<Q>>>::visit(
            tables,
            kind,
            EndNode {
                root,
                cx,
                number,
                ending,
            },
        );
        self.forget(number);
    }

    /// End every node below `top`, each after the nodes below it, for
    /// `ending`.
    pub(super) fn end_below(&mut self, top: u32, ending: Ending) {
        let order = self.post_order(top);
        for n in order.into_iter().filter(|&n| n != top) {
            self.end_by_kind(n, ending);
        }
    }

    /// End the subtree at the node `number` of the kind `K` (at `I`,
    /// inferred) for `reason`, as [`end_at`](Self::end_at) does.
    pub(crate) fn end_for<K, I>(&mut self, number: u32, reason: &IxEndReason<Q, I>)
    where
        KindsOf<Q>: Selector<K, I>,
        I: PackIndex,
    {
        self.end_at::<I>(number, reason);
    }

    /// End the subtree at the node `number` of the kind at `I` for
    /// `reason`: the nodes below it first, for [`EndCause::Above`], then
    /// the node itself, for [`EndCause::Reason`].
    pub(crate) fn end_at<I: PackIndex>(&mut self, number: u32, reason: &IxEndReason<Q, I>) {
        let Some(fk) = ix_table::<Q, I>(&mut self.tables)
            .nodes
            .get(&number)
            .map(|e| e.fk.clone())
        else {
            return;
        };
        self.end_with::<IxDesc<Q, I>, IxOps<Q, I>>(number, &fk, EndCause::Reason(reason));
    }

    /// End the subtree at the node `number` of the kind `D`, proved as
    /// `O`, at `fk`: the nodes below it first, for [`EndCause::Above`],
    /// then the node itself, for `cause`.
    pub(crate) fn end_with<D: KindDesc<Q>, O: KindOps<Q, D>>(
        &mut self,
        number: u32,
        fk: &D::Fk,
        cause: EndCause<'_, <D::Kind as NodeKind>::EndReason>,
    ) {
        if self.nodes.live(number).is_none() {
            return;
        }
        self.end_below(number, Ending::Above);
        let Some(kind) = self.nodes.live(number).map(|n| n.kind) else {
            return;
        };
        // The node is in the tree: the reaction is fired there.
        let _ = O::ending(&mut self.root, fk, &mut self.cx);
        let _ = <Q::Pack as PackList<Q, KindsOf<Q>>>::visit(
            &mut self.tables,
            kind,
            DropCalls { number },
        );
        <O::Comps as CompList<Q, D, D::Comps>>::end_all(&mut self.root, fk, &mut self.cx, cause);
        // The root stays: it is the driver's, and goes with it.
        let _ = O::remove(&mut self.root, fk);
        self.forget(number);
    }

    /// The node `number` moved under `parent`.
    pub(crate) fn reparent(&mut self, number: u32, parent: u32) {
        let Some(old) = self.nodes.live(number).and_then(|n| n.parent) else {
            return;
        };
        if let Some(old) = self.nodes.live_mut(old) {
            old.children.retain(|&c| c != number);
        }
        if let Some(new) = self.nodes.live_mut(parent) {
            new.children.push(number);
        }
        if let Some(n) = self.nodes.live_mut(number) {
            n.parent = Some(parent);
        }
    }

    /// The driver is dropped: every node ends, each after the nodes below
    /// it, for [`EndCause::Dropped`]; what is put in meanwhile is
    /// dropped.
    pub(super) fn end_dropped(&mut self) {
        let Some(root) = self.nodes.iter_live().find(|(_, n)| n.parent.is_none()) else {
            return;
        };
        let root = root.0;
        self.read_clock();
        self.end_below(root, Ending::Dropped);
        self.end_by_kind(root, Ending::Dropped);
        request_lists::<Q>(&mut self.cx).clear_all();
        drop(take_wakes::<Q>(&mut self.cx));
        self.ready.clear();
        self.settles.clear();
    }
}

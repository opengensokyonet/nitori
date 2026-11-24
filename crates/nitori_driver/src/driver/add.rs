//! Adding a node: its key checked, its shares built, put in the tree,
//! given a number, its calls started and woken, and `NodeStarted` fired.
use alloc::{sync::Arc, vec, vec::Vec};

use frunk_core::hlist::HNil;
#[cfg(test)]
use frunk_core::hlist::Selector;

use super::{
    Core,
    nodes::LiveNode,
    ready::{Item, Target},
    table::KindEntry,
    wake::NodeHead,
};
use crate::call::{
    __pack::{CallList, KindDesc, KindList, KindOps, PackIndex, PackList},
    CallHost, Composition, KindsOf, PriorityOf,
    pack_at::{IxDesc, IxFk, IxOps, IxStart, ix_table},
};

/// Why a node was not added.
///
/// Crate-internal until the edit handle gives its own errors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AddRefused {
    /// Its parent is not in the tree.
    NoParent,
    /// Its parent keeps another node of its kind under its key.
    Occupied,
}

impl<Q: Composition> Core<Q> {
    /// Keep the node of the kind `kind` just put in the tree under
    /// `number`, its serial `serial` and its parent `parent`, with
    /// `count` calls, every call woken at `p`.
    fn keep(
        &mut self,
        number: u32,
        kind: usize,
        serial: u64,
        parent: Option<u32>,
        count: u32,
        p: PriorityOf<Q>,
    ) {
        let head = Arc::new(NodeHead::new(number as usize));
        let hosts: Vec<CallHost<PriorityOf<Q>>> = (0..count)
            .map(|j| CallHost::new(p, head.clone(), 1 << j))
            .collect();
        self.nodes.put(
            number,
            LiveNode {
                kind,
                serial,
                head,
                parent,
                children: Vec::new(),
                hosts,
                wakers: vec![None; count as usize],
            },
        );
        if let Some(parent) = parent.and_then(|n| self.nodes.live_mut(n)) {
            parent.children.push(number);
        }
    }

    /// Wake every call of the node `number` at `p`.
    fn wake_calls(&mut self, number: u32, count: u32, p: PriorityOf<Q>) {
        for j in 0..count {
            let item = Item {
                node: number,
                target: Target::Call(j),
            };
            self.ready.wake(item, p);
        }
    }

    /// Keep the root, built already from `start`: its calls started from
    /// what is left of it and woken at `p`, then `NodeStarted` fired.
    pub(super) fn add_root(
        &mut self,
        mut start: <crate::call::RootOf<Q> as crate::structure::NodeKind>::Start,
        p: PriorityOf<Q>,
    ) {
        type D<Q> = <KindsOf<Q> as KindList<Q>>::Head;
        type O<Q> = <<Q as Composition>::Pack as PackList<Q, KindsOf<Q>>>::Head;
        let count = <<O<Q> as KindOps<Q, D<Q>>>::Calls as CallList<
            Q,
            D<Q>,
            <D<Q> as KindDesc<Q>>::Calls,
        >>::COUNT;
        let number = self.nodes.free_number();
        self.root.ident.number = number;
        let serial = self.root.ident.serial;
        self.keep(number, 0, serial, None, count, p);
        let calls = <<O<Q> as KindOps<Q, D<Q>>>::Calls as CallList<
            Q,
            D<Q>,
            <D<Q> as KindDesc<Q>>::Calls,
        >>::start_slots(&mut start);
        <Q::Pack as PackList<Q, KindsOf<Q>>>::head_table(&mut self.tables)
            .nodes
            .insert(number, KindEntry { fk: HNil, calls });
        self.wake_calls(number, count, p);
        let _ = <O<Q> as KindOps<Q, D<Q>>>::started(&mut self.root, &HNil, &mut self.cx);
    }

    /// Add a node of the kind `K` (at `I`, inferred), as
    /// [`add_at`](Self::add_at) does.
    #[cfg(test)]
    pub(crate) fn add<K, I>(
        &mut self,
        fk: IxFk<Q, I>,
        start: IxStart<Q, I>,
        p: PriorityOf<Q>,
    ) -> Result<u32, AddRefused>
    where
        KindsOf<Q>: Selector<K, I>,
        I: PackIndex,
    {
        self.add_at::<I>(fk, start, p)
    }

    /// Add a node of the kind at `I` with the full key `fk`, from
    /// `start`: its key checked, its shares built in the order of the
    /// parts, put in the tree, given a number, its calls started from what
    /// is left of `start` and woken at `p`, then `NodeStarted` fired.
    /// Gives its number.
    pub(crate) fn add_at<I: PackIndex>(
        &mut self,
        fk: IxFk<Q, I>,
        mut start: IxStart<Q, I>,
        p: PriorityOf<Q>,
    ) -> Result<u32, AddRefused> {
        type O<Q, I> = IxOps<Q, I>;
        type D<Q, I> = IxDesc<Q, I>;
        match <O<Q, I> as KindOps<Q, D<Q, I>>>::vacant(&self.root, &fk) {
            None => return Err(AddRefused::NoParent),
            Some(false) => return Err(AddRefused::Occupied),
            Some(true) => {}
        }
        let data = <O<Q, I> as KindOps<Q, D<Q, I>>>::build_at(
            &mut self.root,
            &fk,
            &mut start,
            &self.args,
            &mut self.cx,
        )
        .ok_or(AddRefused::NoParent)?;
        let number = self.nodes.free_number();
        let (own, parent) =
            <O<Q, I> as KindOps<Q, D<Q, I>>>::insert(&mut self.root, &fk, data, number)
                .map_err(|_| AddRefused::Occupied)?;
        type Calls<Q, I> = <O<Q, I> as KindOps<Q, D<Q, I>>>::Calls;
        type Cs<Q, I> = <D<Q, I> as KindDesc<Q>>::Calls;
        let count = <Calls<Q, I> as CallList<Q, D<Q, I>, Cs<Q, I>>>::COUNT;
        self.keep(number, I::N, own.serial, Some(parent.number), count, p);
        let calls = <Calls<Q, I> as CallList<Q, D<Q, I>, Cs<Q, I>>>::start_slots(&mut start);
        ix_table::<Q, I>(&mut self.tables).nodes.insert(
            number,
            KindEntry {
                fk: fk.clone(),
                calls,
            },
        );
        self.wake_calls(number, count, p);
        let _ = <O<Q, I> as KindOps<Q, D<Q, I>>>::started(&mut self.root, &fk, &mut self.cx);
        Ok(number)
    }
}

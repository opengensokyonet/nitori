//! The driver's external operation: a lending at a node by its key
//! address, from outside every call.
use frunk_core::hlist::Selector;

use super::{Driver, Phase};
use crate::{
    call::{
        __pack::PackIndex,
        Composition, ContextOf, KindsOf, PartsOf, RootOf,
        pack_at::{IxFk, ix_table},
    },
    lend::entry::Lends,
    need::{Need, View},
    structure::{KeyAddress, NodeKind, resolve_ident},
};

/// Why the driver did not lend.
#[derive(Debug, snafu::Snafu, PartialEq, Eq, Clone, Copy)]
#[snafu(module)]
pub enum OperateError {
    /// The root ended: the driver is done.
    #[snafu(display("the driver is done: its root ended"))]
    Ended,
    /// A component's operation or a call panicked through an entry before.
    #[snafu(display("an operation or a call panicked in the driver before"))]
    Panicked,
}

impl<Q: Composition> Driver<Q> {
    /// Lend the needs `N` at the node of the kind `K` at `at` to `f`,
    /// as a call is lent its needs; the positions `I` and `P` are
    /// inferred. The requests and wakes put in are handled after it, as
    /// after a call's poll, and the driver's waker is woken when anything
    /// is ready to run. `None` when no node is at `at`.
    #[allow(private_bounds)]
    pub fn operate<K, N, I, P, O>(
        &mut self,
        at: &KeyAddress<RootOf<Q>, K>,
        f: impl for<'v> FnOnce(View<'v, N, P>) -> O,
    ) -> Result<Option<O>, OperateError>
    where
        K: NodeKind,
        crate::call::__pack::Desc<K>: crate::call::__pack::KindDesc<Q, Kind = K>,
        KindsOf<Q>: Selector<K, I>,
        I: PackIndex,
        N: Need + Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, P, Fk = IxFk<Q, I>>,
        P: crate::need::Pos,
    {
        if matches!(self.phase, Phase::Ended) {
            return Err(OperateError::Ended);
        }
        let Some(core) = self.enter() else {
            return Err(match self.phase {
                Phase::Ended => OperateError::Ended,
                Phase::Live(_) | Phase::Panicked => OperateError::Panicked,
            });
        };
        let found = resolve_ident(&core.root, at.route())
            .filter(|i| {
                core.nodes
                    .live(i.number)
                    .is_some_and(|n| n.serial == i.serial)
            })
            .and_then(|i| {
                ix_table::<Q, I>(&mut core.tables)
                    .nodes
                    .get(&i.number)
                    .map(|e| e.fk.clone())
            });
        let out = found.and_then(|fk| {
            core.read_clock();
            let super::Core { root, cx, .. } = &mut *core;
            <N as Lends<_, _, K, _, _, P>>::lend_to(root, &fk, cx, f)
        });
        core.after_lend();
        if !core.ready.is_empty()
            && let Some(w) = &core.outer
        {
            w.wake_by_ref();
        }
        // The root may have ended in what the lending put in: the driver
        // is done then, and its next poll is ready.
        self.leave();
        Ok(out)
    }
}

/// The way down to the kind `K` of `Q`'s structure.
type IlOf<Q, K> = <crate::call::__pack::Desc<K> as crate::call::__pack::KindDesc<Q>>::Il;

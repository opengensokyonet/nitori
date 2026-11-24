//! What the driver does at a node it keeps by number, whichever its kind:
//! poll one of its calls, wake one of its components, end it.
use core::task;

use crate::{
    call::{
        __pack::{CallList, CompList, KindDesc, KindOps, KindVisit, SlotPoll, TableOf},
        CallHost, Composition, ContextOf, PriorityOf, RootNode,
    },
    component::EndCause,
};

/// Poll the call at `j` of the node `number`.
pub(super) struct PollCall<'a, 'w, Q: Composition> {
    pub(super) root: &'a mut RootNode<Q>,
    pub(super) cx: &'a mut ContextOf<Q>,
    pub(super) number: u32,
    pub(super) j: u32,
    pub(super) host: &'a mut CallHost<PriorityOf<Q>>,
    pub(super) wcx: &'a mut task::Context<'w>,
}
impl<Q: Composition> KindVisit<Q> for PollCall<'_, '_, Q> {
    type Out = SlotPoll;
    fn visit<D: KindDesc<Q>, O: KindOps<Q, D>>(self, table: &mut TableOf<Q, D, O>) -> SlotPoll {
        let Some(e) = table.nodes.get_mut(&self.number) else {
            return SlotPoll::NotThere;
        };
        <O::Calls as CallList<Q, D, D::Calls>>::poll_slot(
            self.root,
            &e.fk,
            self.cx,
            &mut e.calls,
            self.j,
            self.host,
            self.wcx,
        )
    }
}

/// Wake the component numbered `component` of the node `number`.
pub(super) struct WakeComponent<'a, Q: Composition> {
    pub(super) root: &'a mut RootNode<Q>,
    pub(super) cx: &'a mut ContextOf<Q>,
    pub(super) number: u32,
    pub(super) component: u64,
}
impl<Q: Composition> KindVisit<Q> for WakeComponent<'_, Q> {
    type Out = ();
    fn visit<D: KindDesc<Q>, O: KindOps<Q, D>>(self, table: &mut TableOf<Q, D, O>) {
        let Some(e) = table.nodes.get(&self.number) else {
            return;
        };
        // A waker of a component the node does not have wakes nothing.
        let _ = <O::Comps as CompList<Q, D, D::Comps>>::wake_number(
            self.root,
            &e.fk,
            self.cx,
            self.component,
        );
    }
}

/// Why a node ends, when it is not the one a request ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ending {
    /// A node above it ended.
    Above,
    /// The driver is dropped.
    Dropped,
}

/// End the node `number`, for `ending`.
pub(super) struct EndNode<'a, Q: Composition> {
    pub(super) root: &'a mut RootNode<Q>,
    pub(super) cx: &'a mut ContextOf<Q>,
    pub(super) number: u32,
    pub(super) ending: Ending,
}
impl<Q: Composition> KindVisit<Q> for EndNode<'_, Q> {
    type Out = ();
    fn visit<D: KindDesc<Q>, O: KindOps<Q, D>>(self, table: &mut TableOf<Q, D, O>) {
        let cause = match self.ending {
            Ending::Above => EndCause::Above,
            Ending::Dropped => EndCause::Dropped,
        };
        end_one::<Q, D, O>(self.root, self.cx, table, self.number, cause);
    }
}

/// End the node `number` of the kind `D`, proved as `O`, for `cause`:
/// fire [`NodeEnding`](crate::react::NodeEnding) at it while it is in
/// the tree, drop its calls, end its components in the order of the
/// parts, and take it out of the tree. The nodes below it ended before.
pub(super) fn end_one<Q: Composition, D: KindDesc<Q>, O: KindOps<Q, D>>(
    root: &mut RootNode<Q>,
    cx: &mut ContextOf<Q>,
    table: &mut TableOf<Q, D, O>,
    number: u32,
    cause: EndCause<'_, <D::Kind as crate::structure::NodeKind>::EndReason>,
) {
    let Some(e) = table.nodes.get(&number) else {
        return;
    };
    let fk = e.fk.clone();
    // The node is in the tree: the reaction is fired there.
    let _ = O::ending(root, &fk, cx);
    drop(table.nodes.remove(&number));
    <O::Comps as CompList<Q, D, D::Comps>>::end_all(root, &fk, cx, cause);
    // The root stays: it is the driver's, and goes with it.
    let _ = O::remove(root, &fk);
}

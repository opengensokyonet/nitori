//! The waker of a component: what is woken to call it again.
use core::marker::PhantomData;

use super::{MemberNeed, Need, Pos, Shape};
use crate::structure::NodeIdent;

/// The waker of the component `X`, without borrowing `X`: of the one node
/// from the lend point up to the root that has it, or of each member
/// inside an [`Under`](super::Under).
///
/// It takes nothing of the node's data, so it does not overlap any need,
/// [`Mut`](super::Mut)`<X>` among them. A component's own waker is
/// `WakerOf<` its own type `>`. No node on the way up having `X`, or more
/// than one, does not compile.
///
/// ```
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(
///     LinkPath,
///     HList![Mut<Planner>, WakerOf<Planner>, WakerOf<ConnCore>, Under<ConnCore, WakerOf<LinkCore>>]
/// );
/// ```
pub struct WakerOf<X>(PhantomData<fn() -> X>);
impl<X: 'static> Need for WakerOf<X> {
    type View<'t, P: Pos> = ComponentWaker;
}
impl<X: 'static> MemberNeed for WakerOf<X> {
    type Collected<'t, P: Pos> = <P::Shape as Shape>::Out<Self::View<'t, P::Inner>>;
}

/// What [`WakerOf`]`<X>` is lent as: the waker of the component `X` of
/// one node, which the driver made.
///
/// It names the node and the component, and holds nothing of either: it
/// is copied freely, compared, and sent to another thread. Two are equal
/// when they are of the same component of the same node. A node that
/// ended is not woken by it, nor is another node built later under the
/// same key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ComponentWaker {
    node: NodeIdent,
    component: u64,
}
impl ComponentWaker {
    /// The waker of the component numbered `component` of the node
    /// `node`.
    pub(crate) fn new(node: NodeIdent, component: u64) -> Self {
        Self { node, component }
    }
    /// The node it wakes.
    pub(crate) fn node(&self) -> NodeIdent {
        self.node
    }
    /// The number of the component it wakes.
    pub(crate) fn component(&self) -> u64 {
        self.component
    }
}

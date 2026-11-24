//! The waker of a component: what is woken to call it again.
use core::marker::PhantomData;

use super::{MemberNeed, Need, Pos, Shape};

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
    type View<'t, P: Pos> = P::Waker;
}
impl<X: 'static> MemberNeed for WakerOf<X> {
    type Collected<'t, P: Pos> = <P::Shape as Shape>::Out<Self::View<'t, P::Inner>>;
}

/// What [`WakerOf`]`<X>` is lent as: the waker of the component `X` of
/// the node of the kind at the route `Rt` with the full key `F`.
///
/// This type is provisional: it is the node's identity, and two are equal
/// when they are of the same node.
pub struct ComponentWaker<X, Rt, F> {
    node: F,
    _of: PhantomData<fn() -> (X, Rt)>,
}
impl<X, Rt, F> ComponentWaker<X, Rt, F> {
    /// The waker of `X` of the node with the full key `node`.
    pub(crate) fn new(node: F) -> Self {
        Self {
            node,
            _of: PhantomData,
        }
    }
}
impl<X, Rt, F: Clone> Clone for ComponentWaker<X, Rt, F> {
    fn clone(&self) -> Self {
        Self::new(self.node.clone())
    }
}
impl<X, Rt, F: PartialEq> PartialEq for ComponentWaker<X, Rt, F> {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
    }
}
impl<X, Rt, F: Eq> Eq for ComponentWaker<X, Rt, F> {}
impl<X, Rt, F: core::fmt::Debug> core::fmt::Debug for ComponentWaker<X, Rt, F> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("ComponentWaker").field(&self.node).finish()
    }
}

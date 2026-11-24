//! Capabilities taken as needs.
use core::marker::PhantomData;

use super::{Need, Pos};
use crate::capability::Capability;

/// The capability `C`: its needs, taken around the lend point and proved
/// with the other needs, put together into its object
/// ([`Capability::Object`]); for a
/// [`ReactionPoint`](crate::react::ReactionPoint), its reactions, fired
/// in turn ([`LentReactions`](crate::react::LentReactions)).
///
/// The capability declares no node to start from; taken at another node,
/// it is in an [`At`](super::At): `At<B, Cap<C>>` takes it there, and
/// `Under<A, At<X, Cap<C>>>` at each member; `Under<A, Cap<C>>` is not a
/// need. A `Cap` and another need that take the same component, one of
/// them exclusive, overlap:
///
/// ```compile_fail,E0277
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(LinkPath, HList![Cap<Planning>, Ref<Planner>]);
/// ```
///
/// ```
/// # use nitori_driver::needs;
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(LinkPath, HList![Cap<Planning>, Ref<ConnCore>]);
/// lends!(Recovery, HList![At<Planner, Cap<Planning>>, Mut<Congestion>]);
/// lends!(LinkPath, needs![Ref<ConnCore>, (Cap<Planning> + Ref<Planner>)]);
/// lends!(Conn, HList![Under<ConnCore, At<Planner, Cap<Planning>>>]);
/// ```
///
/// ```compile_fail,E0277
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(Conn, HList![Under<ConnCore, Cap<Planning>>]);
/// ```
pub struct Cap<C>(PhantomData<fn() -> C>);
impl<C: Capability> Need for Cap<C> {
    type View<'t, P: Pos> = C::Object<'t, P::Inner>;
}

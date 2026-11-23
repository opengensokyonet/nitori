//! The hooks of every share of the parts at a place, part by part.
use alloc::vec::Vec;
use core::task::Waker;

use frunk_core::hlist::{HCons, HNil};

use super::component::P_;
use super::{
    CompInfo, DriverComposition, InstantOf, ParentOf, PlaceKey, PlaceKeyOf, RequestsOf, RootNode,
    ShareOps, TargetOf,
};

use crate::{
    call::{OutboxOf, RootOf},
    event::Triggered,
    hooks::{AbortCause, ArgsOf, EndCause, EndReasonOf, StartOf},
    lend::LendError,
    structure::{ParentPath, PathFrom, RemainsList, RemainsOf, Share, Shares},
};

/// The hooks of every share of the parts `Ps` at `X`, part by part;
/// `Self` is their positions. An index `c` counts the components of the
/// place, part by part.
#[allow(clippy::type_complexity, missing_docs)]
pub trait PartsOps<
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>>,
    Ps,
> where
    Ps: Shares<TargetOf<Q, X>> + RemainsList<TargetOf<Q, X>>,
{
    fn info(out: &mut Vec<CompInfo>);
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &PlaceKeyOf<Q, X>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, X>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<<Ps as Shares<TargetOf<Q, X>>>::Data, LendError>;
    fn deadline(root: &mut RootNode<Q>, at: X, c: usize, o: &OutboxOf<Q>) -> Option<InstantOf<Q>>;
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>>;
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>>;
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>>;
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &OutboxOf<Q>);
    fn end(
        root: &mut RootNode<Q>,
        at: X,
        cause: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    );
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut RemainsOf<Ps, TargetOf<Q, X>>,
        o: &OutboxOf<Q>,
    );
}
impl<Q: DriverComposition, X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>>>
    PartsOps<Q, X, HNil> for HNil
{
    fn info(_: &mut Vec<CompInfo>) {}
    fn build(
        _: &mut Option<&mut RootNode<Q>>,
        _: &ParentOf<X>,
        _: &PlaceKeyOf<Q, X>,
        _: &ArgsOf<P_<Q>>,
        _: &mut StartOf<TargetOf<Q, X>>,
        _: &OutboxOf<Q>,
    ) -> Result<HNil, LendError> {
        Ok(HNil)
    }
    fn deadline(_: &mut RootNode<Q>, _: X, _: usize, _: &OutboxOf<Q>) -> Option<InstantOf<Q>> {
        None
    }
    fn expire(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: InstantOf<Q>,
        _: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn on_events(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: Triggered<'_>,
        _: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn before_calls(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: &Waker,
        _: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        None
    }
    fn aborted(_: &mut RootNode<Q>, _: X, _: AbortCause, _: &OutboxOf<Q>) {}
    fn end(
        _: &mut RootNode<Q>,
        _: X,
        _: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        _: &OutboxOf<Q>,
    ) {
    }
    fn remains(_: &mut RootNode<Q>, _: X, _: &mut HNil, _: &OutboxOf<Q>) {}
}
impl<Q, X, O, Ps, SP, PL> PartsOps<Q, X, HCons<O, Ps>> for HCons<SP, PL>
where
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>> + Copy,
    O: Share<TargetOf<Q, X>> + 'static,
    Ps: Shares<TargetOf<Q, X>> + RemainsList<TargetOf<Q, X>>,
    SP: ShareOps<Q, X, O, O::Data>,
    PL: PartsOps<Q, X, Ps>,
{
    fn info(out: &mut Vec<CompInfo>) {
        SP::info(out);
        PL::info(out);
    }
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &PlaceKeyOf<Q, X>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, X>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<HCons<O::Data, <Ps as Shares<TargetOf<Q, X>>>::Data>, LendError> {
        Ok(HCons {
            head: SP::build(root, parent, key, args, start, outbox)?,
            tail: PL::build(root, parent, key, args, start, outbox)?,
        })
    }
    fn deadline(root: &mut RootNode<Q>, at: X, c: usize, o: &OutboxOf<Q>) -> Option<InstantOf<Q>> {
        match c.checked_sub(SP::LEN) {
            None => SP::deadline(root, at, c, o).flatten(),
            Some(c) => PL::deadline(root, at, c, o),
        }
    }
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        match c.checked_sub(SP::LEN) {
            None => SP::expire(root, at, c, now, o).flatten(),
            Some(c) => PL::expire(root, at, c, now, o),
        }
    }
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        match c.checked_sub(SP::LEN) {
            None => SP::on_events(root, at, c, events, o).flatten(),
            Some(c) => PL::on_events(root, at, c, events, o),
        }
    }
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<RequestsOf<Q>> {
        match c.checked_sub(SP::LEN) {
            None => SP::before_calls(root, at, c, waker, o).flatten(),
            Some(c) => PL::before_calls(root, at, c, waker, o),
        }
    }
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &OutboxOf<Q>) {
        SP::aborted(root, at, cause, o);
        PL::aborted(root, at, cause, o);
    }
    fn end(
        root: &mut RootNode<Q>,
        at: X,
        cause: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    ) {
        SP::end(root, at, cause, o);
        PL::end(root, at, cause, o);
    }
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut HCons<O::Remains, RemainsOf<Ps, TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    ) {
        SP::remains(root, at, &mut out.head, o);
        PL::remains(root, at, &mut out.tail, o);
    }
}

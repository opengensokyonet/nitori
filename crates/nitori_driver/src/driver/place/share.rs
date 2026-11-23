//! The hooks of the components of one share, one by one.
use alloc::vec::Vec;
use core::{any::TypeId, task::Waker};

use frunk_core::hlist::{HCons, HNil};

use super::component::P_;
use super::{
    CompInfo, CompLend, DriverComposition, InstantOf, ParentOf, PlaceKey, PlaceKeyOf, RequestsOf,
    RootNode, TargetOf,
};

use crate::{
    call::{OutboxOf, RootOf},
    event::Triggered,
    hooks::{AbortCause, ArgsOf, EndCause, EndReasonOf, Hooks, StartOf, Timing},
    lend::LendError,
    structure::{ParentPath, PathFrom, Share},
};

/// What a hook's lend came to. The driver lends a hook only at a node it
/// has, so the lend finds it: debug builds check it.
fn lent<T>(r: Result<T, LendError>) -> Option<T> {
    debug_assert!(
        r.is_ok(),
        "a hook is lent at a node the driver has, so the lend finds it"
    );
    r.ok()
}
trait Lent<T> {
    fn lent(self) -> Option<T>;
}
impl<T> Lent<T> for Result<T, LendError> {
    fn lent(self) -> Option<T> {
        lent(self)
    }
}

/// The hooks of the components `Cs` of the share of the part `O` at `X`,
/// one by one; `Self` is their positions. An index `c` counts the share's
/// components; a hook at an index past them gives `None`.
#[allow(clippy::type_complexity, missing_docs)]
pub trait ShareOps<
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>>,
    O,
    Cs,
>
{
    /// How many components.
    const LEN: usize;
    fn info(out: &mut Vec<CompInfo>);
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &PlaceKeyOf<Q, X>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, X>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<Cs, LendError>;
    fn deadline(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        o: &OutboxOf<Q>,
    ) -> Option<Option<InstantOf<Q>>>;
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>>;
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>>;
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>>;
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
        out: &mut <O as Share<TargetOf<Q, X>>>::Remains,
        o: &OutboxOf<Q>,
    ) where
        O: Share<TargetOf<Q, X>>;
}
impl<Q: DriverComposition, X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>>, O>
    ShareOps<Q, X, O, HNil> for HNil
{
    const LEN: usize = 0;
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
    fn deadline(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: &OutboxOf<Q>,
    ) -> Option<Option<InstantOf<Q>>> {
        None
    }
    fn expire(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: InstantOf<Q>,
        _: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        None
    }
    fn on_events(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: Triggered<'_>,
        _: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        None
    }
    fn before_calls(
        _: &mut RootNode<Q>,
        _: X,
        _: usize,
        _: &Waker,
        _: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
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
    fn remains(
        _: &mut RootNode<Q>,
        _: X,
        _: &mut <O as Share<TargetOf<Q, X>>>::Remains,
        _: &OutboxOf<Q>,
    ) where
        O: Share<TargetOf<Q, X>>,
    {
    }
}
impl<Q, X, O, C, Cs, CP, SP> ShareOps<Q, X, O, HCons<C, Cs>> for HCons<CP, SP>
where
    Q: DriverComposition,
    X: PathFrom<RootOf<Q>> + ParentPath + PlaceKey<RootOf<Q>> + Copy,
    C: Hooks<O, P_<Q>, TargetOf<Q, X>>,
    C::Timing: Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>,
    CP: for<'a> CompLend<'a, Q, X, O, C>,
    SP: ShareOps<Q, X, O, Cs>,
{
    const LEN: usize = 1 + SP::LEN;
    fn info(out: &mut Vec<CompInfo>) {
        let mut subscriptions = Vec::new();
        <CP as CompLend<'_, Q, X, O, C>>::subscriptions(&mut subscriptions);
        let mut writes = Vec::new();
        <CP as CompLend<'_, Q, X, O, C>>::writes(&mut writes);
        let mut build_writes = Vec::new();
        <CP as CompLend<'_, Q, X, O, C>>::build_writes(&mut build_writes);
        out.push(CompInfo {
            build_writes,
            type_id: TypeId::of::<C>(),
            timed: <C::Timing as Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>>::TIMED,
            acts_before_calls: C::ACTS_BEFORE_CALLS,
            subscriptions,
            writes,
        });
        SP::info(out);
    }
    fn build(
        root: &mut Option<&mut RootNode<Q>>,
        parent: &ParentOf<X>,
        key: &PlaceKeyOf<Q, X>,
        args: &ArgsOf<P_<Q>>,
        start: &mut StartOf<TargetOf<Q, X>>,
        outbox: &OutboxOf<Q>,
    ) -> Result<HCons<C, Cs>, LendError> {
        let head = <CP as CompLend<'_, Q, X, O, C>>::lend_parent(
            root.as_deref_mut(),
            parent,
            key,
            outbox,
            |view| C::build::<<CP as CompLend<'_, Q, X, O, C>>::BuildPositions>(view, args, start),
        )?;
        Ok(HCons {
            head,
            tail: SP::build(root, parent, key, args, start, outbox)?,
        })
    }
    fn deadline(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        o: &OutboxOf<Q>,
    ) -> Option<Option<InstantOf<Q>>> {
        if c > 0 {
            return SP::deadline(root, at, c - 1, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| {
                <C::Timing as Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>>::deadline(&view)
            })
            .lent()
            .flatten(),
        )
    }
    fn expire(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        now: InstantOf<Q>,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        if c > 0 {
            return SP::expire(root, at, c - 1, now, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| {
                <C::Timing as Timing<C, O, P_<Q>, TargetOf<Q, X>, InstantOf<Q>>>::expire(view, now)
            })
            .lent()
            .flatten()
            .map(<CP as CompLend<'_, Q, X, O, C>>::returned),
        )
    }
    fn on_events(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        events: Triggered<'_>,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        if c > 0 {
            return SP::on_events(root, at, c - 1, events, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| C::on_events(events, view))
                .lent()
                .flatten()
                .map(<CP as CompLend<'_, Q, X, O, C>>::returned),
        )
    }
    fn before_calls(
        root: &mut RootNode<Q>,
        at: X,
        c: usize,
        waker: &Waker,
        o: &OutboxOf<Q>,
    ) -> Option<Option<RequestsOf<Q>>> {
        if c > 0 {
            return SP::before_calls(root, at, c - 1, waker, o);
        }
        Some(
            <CP as CompLend<'_, Q, X, O, C>>::lend(root, at, o, |view| {
                C::before_calls(view, waker)
            })
            .lent()
            .flatten()
            .map(<CP as CompLend<'_, Q, X, O, C>>::returned),
        )
    }
    fn aborted(root: &mut RootNode<Q>, at: X, cause: AbortCause, o: &OutboxOf<Q>) {
        lent(<CP as CompLend<'_, Q, X, O, C>>::lend(
            root,
            at,
            o,
            |view| C::aborted(view, cause),
        ));
        SP::aborted(root, at, cause, o);
    }
    fn end(
        root: &mut RootNode<Q>,
        at: X,
        cause: EndCause<'_, EndReasonOf<TargetOf<Q, X>>>,
        o: &OutboxOf<Q>,
    ) {
        lent(<CP as CompLend<'_, Q, X, O, C>>::lend(
            root,
            at,
            o,
            |view| C::end(view, cause),
        ));
        SP::end(root, at, cause, o);
    }
    fn remains(
        root: &mut RootNode<Q>,
        at: X,
        out: &mut <O as Share<TargetOf<Q, X>>>::Remains,
        o: &OutboxOf<Q>,
    ) where
        O: Share<TargetOf<Q, X>>,
    {
        lent(<CP as CompLend<'_, Q, X, O, C>>::lend(
            root,
            at,
            o,
            |view| C::remains(view, out),
        ));
        SP::remains(root, at, out, o);
    }
}

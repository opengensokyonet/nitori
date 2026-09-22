#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

use nitori_call::TargetExt as _;
use nitori_call::{BoundCall, CallOn, SyncBoundCall, call, call_closure, run_sync, step_sync};
use std::{
    cell::Cell,
    convert::Infallible,
    future::{Future, pending},
    marker::PhantomPinned,
    ops::CoroutineState,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
};

#[call(sync)]
async fn increment(io: nitori_call::Target<nitori_call::Direct<usize>>, amount: usize) -> usize {
    io.with(|__access| {
        let mut host = __access.into_pin().get_mut().0.as_mut();
        {
            *host += amount;
            *host
        }
    })
    .await
}

#[call(sync)]
async fn twice(io: nitori_call::Target<nitori_call::Direct<usize>>, amount: usize) -> usize {
    io.increment(amount).await;
    io.increment(amount).await
}

#[call(sync, yields = usize)]
async fn sequence(
    io: nitori_call::Target<nitori_call::Direct<Cell<usize>>>,
    count: usize,
) -> Result<usize, &'static str> {
    for value in 0..count {
        io.with(|__access| {
            let host = __access.into_pin().get_mut().0.as_mut();
            host.as_ref().get_ref().set(host.get() + 1)
        })
        .await;
        yield value;
    }
    Err("final error")
}

#[call(yields = Infallible, sync,)]
async fn declared_empty(io: nitori_call::Target<nitori_call::Direct<usize>>) -> usize {
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        *host
    })
    .await
}

#[call(sync)]
async fn wait_forever(io: nitori_call::Target<nitori_call::Direct<usize>>) {
    pending::<()>().await
}

#[call(sync, yields = usize)]
async fn yield_then_wait(io: nitori_call::Target<nitori_call::Direct<usize>>) {
    yield 1;
    pending::<()>().await;
}

fn fail_operation() {
    panic!("operation panic");
}

#[call(sync, yields = usize)]
async fn yield_then_panic(io: nitori_call::Target<nitori_call::Direct<usize>>) {
    yield 1;
    fail_operation();
}

#[call(sync, yields = usize)]
async fn borrowed_local(io: nitori_call::Target<nitori_call::Direct<usize>>) -> usize {
    let values = [3, 5];
    let borrowed = &values[..];
    yield borrowed[0];
    io.with(|__access| {
        let mut host = __access.into_pin().get_mut().0.as_mut();
        *host += borrowed[1]
    })
    .await;
    borrowed[0] + borrowed[1]
}

#[call(sync)]
async fn borrowed_result<'value, Receiver: nitori_call::ReceiverFamily, const LENGTH: usize>(
    io: nitori_call::Target<Receiver>,
    value: &'value [u8; LENGTH],
) -> Result<&'value [u8], &'value [u8]> {
    Ok(value)
}

#[call(sync)]
async fn length<Receiver: AsRef<[u8]> + ?Sized>(
    io: nitori_call::Target<nitori_call::Direct<Receiver>>,
) -> usize {
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        host.as_ref().get_ref().as_ref().len()
    })
    .await
}

#[test]
fn immediate_methods_share_composition_and_keep_async_methods() {
    let mut host = 0usize;
    assert_eq!(host.sync_twice_unpin(2), 4);
    assert_eq!(Pin::new(&mut host).sync_twice(3), 10);
    let mut cx = Context::from_waker(Waker::noop());
    let mut bound = pin!(host.twice_unpin(4));
    assert_eq!(bound.as_mut().poll(&mut cx), Poll::Ready(18));
}

#[test]
fn yields_are_lazy_pull_driven_and_completion_is_delivered_once() {
    let mut host = Cell::new(0usize);
    {
        let events = host.sync_sequence_unpin(2);
        // Construction has not executed the operation: verify through a separate
        // observer in the cancellation test below; here each event is explicit.
        let mut events = pin!(events);
        assert_eq!(events.as_mut().next(), Some(CoroutineState::Yielded(0)));
        assert_eq!(events.as_mut().next(), Some(CoroutineState::Yielded(1)));
        assert_eq!(
            events.as_mut().next(),
            Some(CoroutineState::Complete(Err("final error")))
        );
        assert_eq!(events.as_mut().next(), None);
        assert_eq!(events.as_mut().next(), None);
    }
    assert_eq!(host.get(), 2);
    let mut events = pin!(Pin::new(&mut host).sync_sequence(0));
    assert_eq!(
        events.as_mut().collect::<Vec<_>>(),
        vec![CoroutineState::Complete(Err("final error"))]
    );
}

#[test]
fn explicitly_declared_infallible_still_returns_events() {
    let mut host = 7usize;
    let mut events = pin!(host.sync_declared_empty_unpin());
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Complete(7)));
    assert_eq!(events.as_mut().next(), None);
}

#[test]
fn generics_borrowed_outputs_and_unsized_host_are_preserved() {
    let mut bytes = vec![1, 2, 3];
    let host: &mut (dyn AsRef<[u8]> + Unpin) = &mut bytes;
    let mut view = nitori_call::DirectView(Pin::new(host));
    let host = &mut view;
    assert_eq!(host.sync_length_unpin(), 3);
    let input = [4, 5];
    let output = host.sync_borrowed_result_unpin(&input).unwrap();
    assert_eq!(output.as_ptr(), input.as_ptr());
}

#[test]
fn local_borrow_survives_yield_and_for_iteration_preserves_return() {
    let mut host = 0usize;
    {
        let mut events = pin!(host.sync_borrowed_local_unpin());
        let mut observed = Vec::new();
        for event in events.as_mut() {
            observed.push(event);
        }
        assert_eq!(
            observed,
            vec![CoroutineState::Yielded(3), CoroutineState::Complete(8)]
        );
    }
    assert_eq!(host, 5);
}

#[test]
fn pending_is_a_contract_panic_and_event_adapter_is_poisoned() {
    let mut host = 0usize;
    assert!(catch_unwind(AssertUnwindSafe(|| host.sync_wait_forever_unpin())).is_err());
    let mut events = pin!(host.sync_yield_then_wait_unpin());
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Yielded(1)));
    assert!(catch_unwind(AssertUnwindSafe(|| events.as_mut().next())).is_err());
    assert_eq!(events.as_mut().next(), None);
}

#[test]
fn operation_panic_also_terminates_events() {
    let mut host = 0usize;
    let mut events = pin!(host.sync_yield_then_panic_unpin());
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Yielded(1)));
    assert!(catch_unwind(AssertUnwindSafe(|| events.as_mut().next())).is_err());
    assert_eq!(events.as_mut().next(), None);
}

struct PinnedHost {
    count: Rc<Cell<usize>>,
    _pin: PhantomPinned,
}
#[call(sync, yields = usize)]
async fn counted(io: nitori_call::Target<PinnedHost>) -> usize {
    let count = io
        .with(|__access| {
            let host = __access.into_pin().get_mut().0.as_mut();
            host.count.clone()
        })
        .await;
    count.set(count.get() + 1);
    yield count.get();
    count.set(count.get() + 1);
    count.get()
}
#[call(sync)]
async fn current(io: nitori_call::Target<PinnedHost>) -> usize {
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        host.count.get()
    })
    .await
}

#[test]
fn pinned_non_send_host_laziness_and_cancellation() {
    let count = Rc::new(Cell::new(0));
    let mut host = pin!(PinnedHost {
        count: count.clone(),
        _pin: PhantomPinned
    });
    {
        let events = host.as_mut().sync_counted();
        assert_eq!(count.get(), 0);
        let mut events = pin!(events);
        assert_eq!(events.as_mut().next(), Some(CoroutineState::Yielded(1)));
        assert_eq!(count.get(), 1);
    }
    assert_eq!(host.as_mut().sync_current(), 1);
}

struct Manual;
impl CallOn<nitori_call::Direct<usize>> for Manual {
    type Yield = Infallible;
    type Return = usize;
    fn poll_call<'v>(
        self: Pin<&mut Self>,
        view: Pin<&mut dyn nitori_call::ReceiverScope<'v, Family = nitori_call::Direct<usize>>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Infallible, usize>>
    where
        nitori_call::Direct<usize>: 'v,
    {
        let view = std::task::ready!(view.poll_view(cx));

        let mut host = view.get_mut().0.as_mut();
        *host += 1;
        Poll::Ready(CoroutineState::Complete(*host))
    }
}
#[test]
fn adapters_accept_manual_and_anonymous_operations() {
    let mut host = 0usize;
    assert_eq!(run_sync(Pin::new(&mut host), Manual), 1);
    let mut operation = pin!(Manual);
    assert_eq!(
        step_sync(operation.as_mut(), Pin::new(&mut host)),
        CoroutineState::Complete(2)
    );
    let operation = call_closure!(|io: nitori_call::Target<nitori_call::Direct<usize>>| {
        yield io
            .with(|__access| {
                let host = __access.into_pin().get_mut().0.as_mut();
                *host
            })
            .await;
        9
    });
    let mut events = pin!(SyncBoundCall::new(Pin::new(&mut host), operation));
    assert_eq!(
        events.as_mut().collect::<Vec<_>>(),
        vec![CoroutineState::Yielded(2), CoroutineState::Complete(9)]
    );
}

#[test]
fn async_driver_can_still_wait_for_the_same_operation() {
    let mut host = 0usize;
    let mut bound = pin!(BoundCall::new(Pin::new(&mut host), WaitForever::new()));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(bound.as_mut().poll(&mut cx).is_pending());
}

nitori_call::family_receiver!(impl [] for PinnedHost);

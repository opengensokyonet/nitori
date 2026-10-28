//! Operations written as async closures.
#![cfg(feature = "fn-operation")]
mod support;

use nitori_call::{
    Call, CallExecutor, CallPoll, Direct, Lend, Receiver, ReceiverSlot, ViewLoan, fn_operation,
};
use std::{
    cell::Cell,
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
};
use support::*;

type Memory<'a> = Direct<&'a [u8]>;

fn assert_send<T: Send>(_: &T) {}

/// Pending once, waking itself.
async fn yield_once() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if std::mem::replace(&mut yielded, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await
}

#[test]
fn a_closure_runs_on_one_receiver() {
    let mut input = b"\x02hi".as_slice();
    let operation = fn_operation::<Memory, _>(async |mut io: ReceiverSlot<'_, Memory>| {
        let [length] = read_array::<1, _>(&mut io).await.unwrap();
        let mut output = Vec::new();
        read_exact(&mut io, &mut output, length.into())
            .await
            .unwrap();
        output
    });
    let mut run = pin!(CallExecutor::execute(Pin::new(&mut input), operation));
    assert_eq!(poll_once(run.as_mut()), Poll::Ready(b"hi".to_vec()));
}

#[test]
fn a_closure_runs_on_two_receivers_and_captures() {
    let offset = 10u16;
    let operation = fn_operation::<(Memory, Memory), _>(
        async move |(mut a, mut b): (ReceiverSlot<'_, Memory>, ReceiverSlot<'_, Memory>)| {
            let [x] = read_array::<1, _>(&mut a).await.unwrap();
            let [y] = read_array::<1, _>(&mut b).await.unwrap();
            u16::from(x) + u16::from(y) + offset
        },
    );
    let mut inputs = pin!((b"\x01".as_slice(), b"\x02".as_slice()));
    let mut run = pin!(CallExecutor::execute(inputs.as_mut(), operation));
    assert_eq!(poll_once(run.as_mut()), Poll::Ready(13));
}

/// Bytes shared within one thread.
#[derive(Default)]
struct Local(Rc<Cell<u8>>);
nitori_call::direct_receiver!(impl [] for Local);

#[test]
fn a_closure_over_a_resource_that_is_not_send_is_send() {
    fn get(io: &mut ReceiverSlot<'_, Direct<Local>>) -> impl std::future::Future<Output = u8> {
        std::future::poll_fn(move |cx| {
            nitori_call::ReceiverAccess::poll_access(io, cx, |view, _| {
                Poll::Ready(view.into_pin().get_pin_mut().0.get())
            })
        })
    }
    let operation =
        fn_operation::<Direct<Local>, _>(async |mut io: ReceiverSlot<'_, Direct<Local>>| {
            let first = get(&mut io).await;
            yield_once().await;
            first + get(&mut io).await
        });
    let mut call = Box::pin(Call::new(operation));
    assert_send(&call);
    let mut cx = Context::from_waker(Waker::noop());
    let local = Local(Rc::new(Cell::new(2)));
    {
        let mut local = local;
        let mut view = pin!(Pin::new(&mut local).view());
        assert_eq!(
            call.as_mut()
                .poll_return_with_views(Some(Lend::Fixed(ViewLoan::new(view.as_mut()))), &mut cx),
            CallPoll::Pending
        );
    }
    let result = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut local = Local(Rc::new(Cell::new(5)));
                let mut view = pin!(Pin::new(&mut local).view());
                call.as_mut().poll_return_with_views(
                    Some(Lend::Fixed(ViewLoan::new(view.as_mut()))),
                    &mut Context::from_waker(Waker::noop()),
                )
            })
            .join()
            .unwrap()
    });
    assert_eq!(result, CallPoll::Ready(7));
}

#[test]
fn a_closure_over_a_borrowed_family_is_send() {
    fn build<'a>(_: &'a [u8]) {
        let operation =
            fn_operation::<Memory<'a>, _>(async |mut io: ReceiverSlot<'_, Memory<'a>>| {
                let [x] = read_array::<1, _>(&mut io).await.unwrap();
                yield_once().await;
                let [y] = read_array::<1, _>(&mut io).await.unwrap();
                x + y
            });
        assert_send(&Call::new(operation));
    }
    build(b"");
}

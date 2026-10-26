//! An operation whose body contains a real coroutine keeps it as written.
#![feature(
    impl_trait_in_assoc_type,
    yield_expr,
    coroutines,
    coroutine_trait,
    stmt_expr_attributes
)]
use nitori_call_prototype::{Call, CallPoll, ReceiverAccess, call};
use std::{
    ops::{ControlFlow, Coroutine, CoroutineState},
    pin::{Pin, pin},
    task::{Context, Waker},
};

#[call(yields = u8)]
async fn with_coroutine(io: &mut impl ReceiverAccess<Family: Sized>) -> u8 {
    let _ = io;
    let mut inner = #[coroutine]
    || {
        yield 10;
        20
    };
    // The coroutine's yield is its own; this one is the operation's item.
    let CoroutineState::Yielded(first) = Pin::new(&mut inner).resume(()) else {
        unreachable!()
    };
    yield first + 1;
    let CoroutineState::Complete(last) = Pin::new(&mut inner).resume(()) else {
        unreachable!()
    };
    last
}

#[test]
fn a_coroutine_inside_an_operation_is_untouched() {
    let mut host = 0_u8;
    let mut call = pin!(Call::<nitori_call_prototype::Direct<u8>, _>::new(
        WithCoroutine::new()
    ));
    let mut cx = Context::from_waker(Waker::noop());
    let mut poll = || {
        let view = pin!(nitori_call_prototype::DirectView(Pin::new(&mut host)));
        call.as_mut().poll_with(Some(view), &mut cx)
    };
    assert_eq!(poll(), CallPoll::Ready(ControlFlow::Continue(11)));
    assert_eq!(poll(), CallPoll::Ready(ControlFlow::Break(20)));
}

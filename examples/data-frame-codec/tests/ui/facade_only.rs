// expect: pass
// facade-only
// A crate that enables no features stores, awaits and streams executions.
use futures_core::Stream;
use nitori_call::CallExecutor;
use nitori_io::calls::{ReadArray, ReadArrayHostExt as _};
use std::{future::Future, ops::ControlFlow, pin::{Pin, pin}, task::{Context, Poll, Waker}};
struct Reading<'a> {
    execution: <&'static [u8] as CallExecutor>::Execution<'a, ReadArray<2>>,
}
fn main() {
    let mut input: &'static [u8] = b"abcd";
    let mut cx = Context::from_waker(Waker::noop());
    {
        let reading = Reading { execution: input.read_array_unpin::<2>() };
        let execution = pin!(reading.execution);
        assert!(matches!(execution.poll(&mut cx), Poll::Ready(Ok(bytes)) if bytes == *b"ab"));
    }
    let mut execution = pin!(Pin::new(&mut input).read_array::<2>());
    assert!(matches!(execution.as_mut().poll_next(&mut cx), Poll::Ready(Some(ControlFlow::Break(Ok(bytes)))) if bytes == *b"cd"));
    assert!(matches!(execution.as_mut().poll_next(&mut cx), Poll::Ready(None)));
}

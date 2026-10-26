// expect: pass
// An execution of an emitting operation from another crate is a struct field,
// read as a stream and then awaited for its final value.
use futures_core::Stream;
use nitori_call::CallExecutor;
use nitori_data_frame_codec_example::current_codec::{ReadDataFrame, ReadDataFrameHostExt as _};
use std::{future::Future, num::NonZeroUsize, ops::ControlFlow, pin::Pin, task::{Context, Poll, Waker}};
struct Frame<'a> {
    execution: Pin<Box<<&'static [u8] as CallExecutor>::Execution<'a, ReadDataFrame>>>,
}
fn main() {
    let mut input: &'static [u8] = b"\0\x03abc";
    let mut frame = Frame { execution: Box::pin(input.read_data_frame_unpin(NonZeroUsize::new(2).unwrap())) };
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(frame.execution.as_mut().poll_next(&mut cx), Poll::Ready(Some(ControlFlow::Continue(b"ab")))));
    assert!(matches!(frame.execution.as_mut().poll(&mut cx), Poll::Ready(Ok(3))));
}

//! Compose partial writes and report progress before completing a line.
#![feature(impl_trait_in_assoc_type, yield_expr)]

use futures_core::Stream;
use nitori_call::{ReceiverAccess, call};
use nitori_io::{
    Write,
    calls::{WriteAllError, WriteAllExt as _},
};
use std::{
    ops::ControlFlow,
    pin::pin,
    task::{Context, Poll, Waker},
};

#[call(sync, yields = usize)]
async fn write_line<A: ReceiverAccess<Family: Write>>(
    io: &mut A,
    line: &[u8],
) -> Result<usize, WriteAllError<<A::Family as Write>::Error>> {
    let written = io.write_all(line).await.result?;
    yield written;
    io.write_all(b"\n".as_slice()).await.result?;
    Ok(written + 1)
}

fn main() {
    let mut output = Vec::new();
    {
        // As a stream, the execution delivers the progress item, then the final value.
        let mut events = pin!(output.write_line_unpin(b"hello"));
        let mut cx = Context::from_waker(Waker::noop());
        while let Poll::Ready(Some(event)) = events.as_mut().poll_next(&mut cx) {
            match event {
                ControlFlow::Continue(written) => println!("payload written: {written}"),
                ControlFlow::Break(result) => println!("line written: {}", result.unwrap()),
            }
        }
    }
    // The synchronous method keeps only the final value.
    assert_eq!(output.sync_write_line_unpin(b"world").unwrap(), 6);
    assert_eq!(output, b"hello\nworld\n");
}

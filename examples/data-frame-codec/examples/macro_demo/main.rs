//! Read one DATA frame from built-in slice IO, handling each chunk as it arrives.
use futures_core::Stream;
use nitori_data_frame_codec_example::current_codec::ReadDataFrameHostExt;
use std::{
    num::NonZeroUsize,
    ops::ControlFlow,
    pin::pin,
    task::{Context, Poll, Waker},
};

fn main() {
    // DATA type, length, payload, then an empty DATA frame left unread.
    let mut input = b"\0\x05hello\0\0".as_slice();
    {
        let mut frame = pin!(input.read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
        // Slice IO never waits, so a no-op waker suffices.
        let mut cx = Context::from_waker(Waker::noop());
        while let Poll::Ready(Some(event)) = frame.as_mut().poll_next(&mut cx) {
            match event {
                ControlFlow::Continue(chunk) => println!("chunk: {chunk:?}"),
                ControlFlow::Break(result) => println!("length: {}", result.unwrap()),
            }
        }
    }
    assert_eq!(input, b"\0\0");
}

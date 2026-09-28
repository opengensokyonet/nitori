//! Read one DATA frame with built-in slice IO and synchronous event iteration.
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

mod codec;

use codec::ReadDataFrameExt;
use std::{num::NonZeroUsize, ops::CoroutineState, pin::pin};

fn main() {
    // DATA type, length, payload, then an empty DATA frame left unread.
    let mut input = b"\0\x05hello\0\0".as_slice();
    {
        let mut frame = pin!(input.sync_read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
        for event in frame.as_mut() {
            match event {
                CoroutineState::Yielded(chunk) => println!("chunk: {chunk:?}"),
                CoroutineState::Complete(result) => println!("length: {}", result.unwrap()),
            }
        }
    }
    assert_eq!(input, b"\0\0");
}

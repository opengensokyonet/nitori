//! Compose IO calls and observe an intermediate value before the final result.
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

use nitori_call::{Target, call};
use nitori_io::{Read, TargetReadExt, calls::ReadBeError};
use std::{ops::CoroutineState, pin::pin};

#[call(sync, yields = u16)]
async fn read_pair<H: Read>(io: Target<H>) -> Result<u32, ReadBeError<H::Error>> {
    let first = io.read_be::<u16>().await?;
    yield first;
    let second = io.read_be::<u16>().await?;
    Ok(u32::from(first) + u32::from(second))
}

fn main() {
    // Slices already implement nitori_io's read capabilities.
    let mut input = b"\x00\x02\x00\x03".as_slice();
    {
        // Memory reads are immediately ready, so no executor is needed.
        let mut events = pin!(input.sync_read_pair_unpin());
        for event in events.as_mut() {
            match event {
                CoroutineState::Yielded(first) => println!("first: {first}"),
                CoroutineState::Complete(result) => println!("sum: {}", result.unwrap()),
            }
        }
    }
    assert!(input.is_empty());
}

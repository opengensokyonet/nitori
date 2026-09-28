//! Named and anonymous calls performing the same IO; see expanded/expansion_demo.rs.
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

use nitori_call::{Direct, Target, call, call_closure, run_sync};
use nitori_io::{Read, TargetReadExt, calls::ReadBeError};
use std::{convert::Infallible, pin::Pin};

#[call(sync)]
async fn read_pair<H: Read>(io: Target<H>) -> Result<u16, ReadBeError<H::Error>> {
    let first = io.read_be::<u8>().await?;
    let second = io.read_be::<u8>().await?;
    Ok(u16::from(first) + u16::from(second))
}

fn main() {
    let mut input = b"\x01\x02".as_slice();
    assert_eq!(input.sync_read_pair_unpin().unwrap(), 3);
    assert!(input.is_empty());

    let mut input = b"\x01\x02".as_slice();
    let operation = call_closure!(
        |io: Target<Direct<&[u8]>>| -> Result<u16, ReadBeError<Infallible>> {
            let first = io.read_be::<u8>().await?;
            let second = io.read_be::<u8>().await?;
            Ok(u16::from(first) + u16::from(second))
        }
    );
    assert_eq!(run_sync(Pin::new(&mut input), operation).unwrap(), 3);
    assert!(input.is_empty());
}

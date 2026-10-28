//! A named operation and a one-off root operation declared in a block,
//! performing the same IO; see expanded/expansion_demo.rs.
#![feature(impl_trait_in_assoc_type)]

use nitori_call::{CallExecutor, ReceiverAccess, call};
use nitori_io::{
    Read,
    calls::{ReadBeError, ReadBeExt as _},
};
use std::{
    convert::Infallible,
    future::Future,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

#[call(sync)]
async fn read_pair<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Result<u16, ReadBeError<F::Error>> {
    let first = io.read_be::<u8>().await?;
    let second = io.read_be::<u8>().await?;
    Ok(u16::from(first) + u16::from(second))
}

fn main() {
    let mut input = b"\x01\x02".as_slice();
    assert_eq!(input.sync_read_pair_unpin().unwrap(), 3);
    assert!(input.is_empty());

    let mut input = b"\x01\x02".as_slice();
    let sum = {
        // The generated items stay inside this block.
        #[call]
        async fn sum<A: ReceiverAccess<Family: Read<Error = Infallible>>>(
            io: &mut A,
        ) -> Result<u16, ReadBeError<Infallible>> {
            let first = io.read_be::<u8>().await?;
            let second = io.read_be::<u8>().await?;
            Ok(u16::from(first) + u16::from(second))
        }
        let execution = pin!(CallExecutor::execute(Pin::new(&mut input), Sum::new()));
        match execution.poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(sum) => sum,
            Poll::Pending => unreachable!("slice IO never waits"),
        }
    };
    assert_eq!(sum.unwrap(), 3);
    assert!(input.is_empty());
}

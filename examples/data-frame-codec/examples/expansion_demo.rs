//! A named operation and a one-off root operation declared in a block,
//! performing the same IO; see expanded/expansion_demo.rs.
#![feature(impl_trait_in_assoc_type)]

use nitori_call::{CallExecutor, ReceiverAccess, call};
use nitori_io::{
    Read,
    calls::{ReadBeError, ReadBeExt as _},
};
use snafu::{ResultExt, Snafu};
use std::{
    convert::Infallible,
    future::Future,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

#[derive(Clone, Copy, Debug)]
enum OperandPosition {
    First,
    Second,
}

#[derive(Debug, Snafu)]
enum ReadPairError<E> {
    #[snafu(display("could not read the {position:?} operand"))]
    Operand {
        position: OperandPosition,
        source: ReadBeError<E>,
    },
}

#[call(sync)]
async fn read_pair<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Result<u16, ReadPairError<F::Error>> {
    let first = io.read_be::<u8>().await.context(OperandSnafu {
        position: OperandPosition::First,
    })?;
    let second = io.read_be::<u8>().await.context(OperandSnafu {
        position: OperandPosition::Second,
    })?;
    Ok(u16::from(first) + u16::from(second))
}

fn main() {
    let mut input = b"\x01\x02".as_slice();
    assert_eq!(input.sync_read_pair_unpin().unwrap(), 3);
    assert!(input.is_empty());

    let mut input = b"\x01\x02".as_slice();
    let sum = {
        // The generated items stay inside this block.
        #[derive(Debug, Snafu)]
        enum SumError {
            #[snafu(display("could not read the {position:?} operand"))]
            Operand {
                position: OperandPosition,
                source: ReadBeError<Infallible>,
            },
        }

        #[call]
        async fn sum<A: ReceiverAccess<Family: Read<Error = Infallible>>>(
            io: &mut A,
        ) -> Result<u16, SumError> {
            let first = io.read_be::<u8>().await.context(OperandSnafu {
                position: OperandPosition::First,
            })?;
            let second = io.read_be::<u8>().await.context(OperandSnafu {
                position: OperandPosition::Second,
            })?;
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

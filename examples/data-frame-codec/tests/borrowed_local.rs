#![feature(impl_trait_in_assoc_type)]

use bytes::{Buf, Bytes};
use nitori_call::{Call, CallPoll, Lend, Receiver, ReceiverAccess, call};
use nitori_io::{Write, WriteExt as _};
use std::convert::Infallible;
use std::{
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

struct Output {
    bytes: Vec<u8>,
    waited: bool,
}

impl Write for Output {
    type Error = Infallible;
    fn poll_write<'visit, Input: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut Input,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut().get_mut();
        if !this.waited {
            this.waited = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.waited = false;
        let count = input.chunk().len().min(2);
        this.bytes.extend_from_slice(&input.chunk()[..count]);
        input.advance(count);
        Poll::Ready(Ok(count))
    }
}

#[call]
async fn write_local_buffer<A: ReceiverAccess<Family: Write<Error = Infallible>>>(
    io: &mut A,
) -> Result<(Bytes, usize, Bytes, usize), Infallible> {
    let mut buf = Bytes::from_static(b"abc");
    let first_count = io.write(&mut buf).await?;
    let tail = buf.clone();
    let second_count = io.write(&mut buf).await?;
    Ok((buf, first_count, tail, second_count))
}

#[test]
fn named_call_reborrows_local_buf_across_pending() {
    let mut output = Output {
        bytes: Vec::new(),
        waited: false,
    };
    let mut call = pin!(Call::<Output, _>::new(WriteLocalBuffer::new()));
    let mut poll = |output: &mut Output| {
        call.as_mut().poll_return_with(
            Some(Lend::Fixed(pin!(Pin::new(output).view()))),
            &mut Context::from_waker(Waker::noop()),
        )
    };
    assert!(matches!(poll(&mut output), CallPoll::Pending));
    assert!(output.bytes.is_empty());
    assert!(matches!(poll(&mut output), CallPoll::Pending));
    assert_eq!(output.bytes, b"ab");
    let CallPoll::Ready(Ok((buf, first_count, tail, second_count))) = poll(&mut output) else {
        panic!("expected completion")
    };
    assert_eq!(first_count, 2);
    assert_eq!(tail.as_ref(), b"c");
    assert_eq!(second_count, 1);
    assert!(buf.is_empty());
    assert_eq!(output.bytes, b"abc");
}

nitori_call::family_receiver!(impl [] for Output);

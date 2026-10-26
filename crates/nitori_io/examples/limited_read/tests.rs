use super::*;
use bytes::BufMut;
use nitori_call::{Call, CallPoll, Direct, Receiver, family_receiver};
use std::{
    convert::Infallible,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

struct Delayed<'a> {
    input: &'a [u8],
    waiting: bool,
}
family_receiver!(impl ['a] for Delayed<'a>);
impl Read for Delayed<'_> {
    type Error = Infallible;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'v,
    {
        let this = host.get_mut().0.as_mut().get_mut();
        if !this.waiting {
            this.waiting = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.waiting = false;
        <Direct<&[u8]> as Read>::poll_read(
            pin!(nitori_call::DirectView(Pin::new(&mut this.input))),
            cx,
            &mut out.limit(1),
        )
    }
}

fn poll<'data>(
    call: Pin<&mut Call<Delayed<'data>, ReadRecord>>,
    host: &mut Delayed<'data>,
) -> CallPoll<Result<[u8; 2], ReadArrayError<Infallible>>> {
    call.poll_return_with(
        Some(pin!(Pin::new(host).view())),
        &mut Context::from_waker(Waker::noop()),
    )
}

#[test]
fn boundary_keeps_the_next_record() {
    super::main();
}

#[test]
fn budget_survives_pending_and_partial_reads() {
    let mut host = Delayed {
        input: b"\x01A\x02BC",
        waiting: false,
    };
    let mut call = pin!(Call::new(ReadRecord::new()));
    assert!(matches!(poll(call.as_mut(), &mut host), CallPoll::Pending));
    assert_eq!(host.input, b"\x01A\x02BC");
    assert!(matches!(poll(call.as_mut(), &mut host), CallPoll::Pending));
    assert_eq!(host.input, b"A\x02BC");
    let CallPoll::Ready(Err(error)) = poll(call.as_mut(), &mut host) else {
        panic!("expected record boundary")
    };
    assert_eq!(error.completed(), 1);
    assert_eq!(host.input, b"\x02BC");
}

#[test]
fn cancellation_preserves_unread_payload() {
    let mut host = Delayed {
        input: b"\x02AB\x02CD",
        waiting: false,
    };
    {
        let mut call = pin!(Call::new(ReadRecord::new()));
        for _ in 0..3 {
            assert!(matches!(poll(call.as_mut(), &mut host), CallPoll::Pending));
        }
    }
    assert_eq!(host.input, b"B\x02CD");
}

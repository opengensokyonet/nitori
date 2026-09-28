use super::*;
use bytes::BufMut;
use nitori_call::{Direct, PollCallExt, family_receiver};
use std::{
    convert::Infallible,
    ops::CoroutineState,
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
    let mut call = pin!(read_record::<Delayed<'_>>());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        call.as_mut()
            .poll_receiver(Pin::new(&mut host), &mut cx)
            .is_pending()
    );
    assert_eq!(host.input, b"\x01A\x02BC");
    assert!(
        call.as_mut()
            .poll_receiver(Pin::new(&mut host), &mut cx)
            .is_pending()
    );
    assert_eq!(host.input, b"A\x02BC");
    let Poll::Ready(CoroutineState::Complete(Err(error))) =
        call.as_mut().poll_receiver(Pin::new(&mut host), &mut cx)
    else {
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
        let mut call = pin!(read_record::<Delayed<'_>>());
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            call.as_mut()
                .poll_receiver(Pin::new(&mut host), &mut cx)
                .is_pending()
        );
        assert!(
            call.as_mut()
                .poll_receiver(Pin::new(&mut host), &mut cx)
                .is_pending()
        );
        assert!(
            call.as_mut()
                .poll_receiver(Pin::new(&mut host), &mut cx)
                .is_pending()
        );
    }
    assert_eq!(host.input, b"B\x02CD");
}

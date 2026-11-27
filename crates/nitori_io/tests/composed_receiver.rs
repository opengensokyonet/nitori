//! A byte budget composed on a receiver: it lives with the receiver, across
//! calls, cancelled ones included, and calls on it compose again.
#![feature(impl_trait_in_assoc_type)]
use bytes::BufMut;
use core::{
    convert::Infallible,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use nitori_call::{Call, CallPoll, Lend, Receiver, ReceiverAccess, call};
use nitori_io::{
    adapters::{Limited, ReadLimit},
    calls::ReadArrayError,
    prelude::*,
};

#[call(sync)]
async fn read_two<F: Read<Metadata: nitori_io::Combine>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<[u8; 2], ReadArrayError<F::Error>> {
    io.read_array::<2>().await.map(|read| read.value)
}

/// Reads two bytes within a budget of one composed in the call, and returns
/// what is left of that budget.
#[call(sync)]
async fn read_two_within_one<F: Read<Metadata: nitori_io::Combine>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (Result<[u8; 2], ReadArrayError<F::Error>>, usize) {
    let mut limited = ReadLimit::new(&mut *io, 1usize);
    let result = limited.read_array::<2>().await.map(|read| read.value);
    (result, limited.into_parts().quota)
}

#[test]
fn the_budget_outlives_each_call() {
    let mut framed = ReadLimit::new(&b"abcdef"[..], 3usize);
    assert_eq!(framed.sync_read_two_unpin().unwrap(), *b"ab");
    assert_eq!(framed.quota(), &1);
    // One byte is left in the budget: the next read of two stops after it.
    let error = framed.sync_read_two_unpin().unwrap_err();
    assert_eq!(error.completed(), 1);
    assert_eq!(framed.quota(), &0);
    let rest = framed.into_parts().io;
    assert_eq!(rest, b"def");
}

#[test]
fn a_call_composes_again_within_the_budget() {
    let mut framed = ReadLimit::new(&b"abcdef"[..], 4usize);
    let (result, inner) = framed.sync_read_two_within_one_unpin();
    assert_eq!(result.unwrap_err().completed(), 1);
    assert_eq!(inner, 0);
    // The byte read through the inner budget is counted by the outer one.
    assert_eq!(framed.quota(), &3);
    assert_eq!(framed.sync_read_two_unpin().unwrap(), *b"bc");
    assert_eq!(framed.get_ref(), &&b"def"[..]);
    assert_eq!(framed.quota(), &1);
}

/// A source that delivers one byte on every second poll.
struct Delayed<'data> {
    bytes: &'data [u8],
    waiting: bool,
}
nitori_call::family_receiver!(impl ['data] for Delayed<'data>);
impl Read for Delayed<'_> {
    type Error = Infallible;
    type Metadata = ();
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<Option<nitori_io::ReadSegment<Self::Metadata>>, Self::Error>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if !this.waiting {
            this.waiting = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.waiting = false;
        let count = out.remaining_mut().min(this.bytes.len()).min(1);
        out.put_slice(&this.bytes[..count]);
        this.bytes = &this.bytes[count..];
        Poll::Ready(Ok(segment(count)))
    }
}

#[test]
fn a_cancelled_call_leaves_the_source_and_the_budget_in_step() {
    let mut framed = ReadLimit::new(
        Delayed {
            bytes: b"abcdef",
            waiting: false,
        },
        4usize,
    );
    let mut call = Box::pin(Call::<Limited<Delayed<'_>, usize>, _>::new(ReadTwo::new()));
    let mut cx = Context::from_waker(Waker::noop());
    // The second poll reads the first byte, then waits for the second.
    for _ in 0..2 {
        let poll = call.as_mut().poll_with(
            Some(Lend::Fixed(pin!(Pin::new(&mut framed).view()))),
            &mut cx,
        );
        assert!(matches!(poll, CallPoll::Pending));
    }
    drop(call);
    let source = framed.get_ref();
    let budget = framed.quota();
    assert_eq!(source.bytes, b"bcdef");
    assert_eq!(budget, &3);
    // A new call goes on from there.
    let mut call = pin!(Call::<Limited<Delayed<'_>, usize>, _>::new(ReadTwo::new()));
    let read = loop {
        let poll = call.as_mut().poll_with(
            Some(Lend::Fixed(pin!(Pin::new(&mut framed).view()))),
            &mut cx,
        );
        match poll {
            CallPoll::Pending => continue,
            CallPoll::Ready(ControlFlow::Break(read)) => break read,
            _ => panic!("the call neither waits nor finishes"),
        }
    };
    assert_eq!(read.unwrap(), *b"bc");
    assert_eq!(framed.quota(), &1);
}

fn segment(count: usize) -> Option<nitori_io::ReadSegment<()>> {
    core::num::NonZeroUsize::new(count).map(|count| nitori_io::ReadSegment {
        count,
        metadata: (),
    })
}

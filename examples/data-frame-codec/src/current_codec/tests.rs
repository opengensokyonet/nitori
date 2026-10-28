use super::*;
use bytes::{BufMut, Bytes};
use futures_core::Stream;
use nitori_call::{Call, CallPoll, Lend, Receiver};
use nitori_io::calls::WriteAll;
use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll, Waker},
};

/// The next event of a stream that never waits.
fn next<S: Stream + ?Sized>(stream: Pin<&mut S>) -> Option<S::Item> {
    match stream.poll_next(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(event) => event,
        Poll::Pending => panic!("memory input does not wait"),
    }
}

#[test]
fn chunks_stop_at_the_frame_boundary() {
    let mut input = b"\0\x05hello\0\0".as_slice();
    {
        let mut frame = pin!(input.read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
        for expected in [b"he".as_slice(), b"ll", b"o"] {
            assert!(
                matches!(next(frame.as_mut()), Some(ControlFlow::Continue(chunk)) if chunk == expected)
            );
        }
        assert!(matches!(
            next(frame.as_mut()),
            Some(ControlFlow::Break(Ok(5)))
        ));
        assert!(next(frame.as_mut()).is_none());
    }
    assert_eq!(input, b"\0\0");
}

#[test]
fn truncated_payload_reports_progress() {
    let mut input = b"\0\x03ab".as_slice();
    let mut frame = pin!(input.read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
    assert!(matches!(
        next(frame.as_mut()),
        Some(ControlFlow::Continue(b"ab"))
    ));
    let Some(ControlFlow::Break(Err(CodecError::Payload { source }))) = next(frame.as_mut()) else {
        panic!("expected an incomplete payload");
    };
    assert_eq!(source.completed(), 2);
}

#[test]
fn multi_byte_length_and_wrong_type() {
    let mut input = b"\0\x40\x01x".as_slice();
    {
        let mut frame = pin!(input.read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
        assert!(matches!(
            next(frame.as_mut()),
            Some(ControlFlow::Continue(b"x"))
        ));
        assert!(matches!(
            next(frame.as_mut()),
            Some(ControlFlow::Break(Ok(1)))
        ));
    }
    assert!(input.is_empty());
    let mut input = b"\x01\0".as_slice();
    assert!(matches!(
        input.sync_read_data_frame_unpin(NonZeroUsize::new(2).unwrap()),
        Err(CodecError::WrongType { actual: 1 })
    ));
}

/// Counts chunk reads and returns at most two bytes per chunk.
struct Source {
    bytes: Bytes,
    reads: usize,
}
impl Read for Source {
    type Error = Infallible;
    fn poll_read<'visit, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        nitori_io::helpers::poll_read_from_chunk(host, cx, out)
    }
}
impl ReadChunk for Source {
    type Chunk = Bytes;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Bytes>, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut().get_mut();
        this.reads += 1;
        if this.bytes.is_empty() {
            return Poll::Ready(Ok(None));
        }
        let count = this.bytes.len().min(maximum.get()).min(2);
        Poll::Ready(Ok(Some(this.bytes.split_to(count))))
    }
}
nitori_call::family_receiver!(impl [] for Source);

/// A parent that forwards the frame's chunks as its own items.
#[call(yields = Bytes)]
async fn forward_frame<A: ReceiverAccess<Family: ReadChunk<Chunk = Bytes, Error = Infallible>>>(
    io: &mut A,
) -> Result<usize, CodecError<Infallible>> {
    let mut frame = pin!(io.read_data_frame(NonZeroUsize::new(3).unwrap()));
    while let Some(event) = frame.as_mut().next().await {
        match event {
            ControlFlow::Continue(chunk) => yield chunk,
            ControlFlow::Break(result) => return result,
        }
    }
    unreachable!("the final value ends the stream")
}

#[test]
fn data_frame_yields_without_reading_ahead_and_keeps_next_frame() {
    let mut source = Source {
        bytes: Bytes::from_static(b"\x00\x05hello\x00\x00"),
        reads: 0,
    };
    let mut call = pin!(Call::new(ForwardFrame::new()));
    let poll = |call: Pin<&mut Call<Source, ForwardFrame>>, source: &mut Source| {
        call.poll_with(
            Some(Lend::Fixed(pin!(Pin::new(source).view()))),
            &mut Context::from_waker(Waker::noop()),
        )
    };
    for (expected, remaining, reads) in [
        (b"he".as_slice(), 5, 3),
        (b"ll".as_slice(), 3, 4),
        (b"o".as_slice(), 2, 5),
    ] {
        let CallPoll::Ready(ControlFlow::Continue(part)) = poll(call.as_mut(), &mut source) else {
            panic!("expected chunk")
        };
        assert_eq!(part.as_ref(), expected);
        assert_eq!((source.bytes.len(), source.reads), (remaining, reads));
    }
    assert!(matches!(
        poll(call.as_mut(), &mut source),
        CallPoll::Ready(ControlFlow::Break(Ok(5)))
    ));
    assert_eq!(source.reads, 5);
}

/// Accepts at most two bytes per write, waiting once before each.
pub(super) struct Output {
    bytes: Vec<u8>,
    waited: bool,
}
impl nitori_io::Write for Output {
    type Error = Infallible;
    fn poll_write<'visit, Input: bytes::Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut Input,
    ) -> Poll<Result<usize, Self::Error>>
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
nitori_call::family_receiver!(impl [] for Output);

#[test]
fn named_write_preserves_partial_progress_across_pending() {
    let mut output = Output {
        bytes: Vec::new(),
        waited: false,
    };
    let mut call = pin!(Call::new(WriteAll::new(Bytes::from_static(b"abc"))));
    let poll = |call: Pin<&mut Call<Output, WriteAll<Bytes>>>, output: &mut Output| {
        call.poll_return_with(
            Some(Lend::Fixed(pin!(Pin::new(output).view()))),
            &mut Context::from_waker(Waker::noop()),
        )
    };
    assert!(matches!(
        poll(call.as_mut(), &mut output),
        CallPoll::Pending
    ));
    assert!(output.bytes.is_empty());
    assert!(matches!(
        poll(call.as_mut(), &mut output),
        CallPoll::Pending
    ));
    assert_eq!(output.bytes, b"ab");
    let CallPoll::Ready(returned) = poll(call.as_mut(), &mut output) else {
        panic!("expected completion")
    };
    returned.unwrap();
    assert_eq!(output.bytes, b"abc");
}

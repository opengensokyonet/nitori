use super::*;
use std::pin::pin;

#[test]
fn chunks_stop_at_the_frame_boundary() {
    let mut input = b"\0\x05hello\0\0".as_slice();
    {
        let mut frame = pin!(input.sync_read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
        for expected in [b"he".as_slice(), b"ll", b"o"] {
            assert!(
                matches!(frame.as_mut().next(), Some(CoroutineState::Yielded(chunk)) if chunk == expected)
            );
        }
        assert!(matches!(
            frame.as_mut().next(),
            Some(CoroutineState::Complete(Ok(5)))
        ));
        assert!(frame.as_mut().next().is_none());
    }
    assert_eq!(input, b"\0\0");
}

#[test]
fn truncated_payload_reports_progress() {
    let mut input = b"\0\x03ab".as_slice();
    let mut frame = pin!(input.sync_read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
    assert!(matches!(
        frame.as_mut().next(),
        Some(CoroutineState::Yielded(b"ab"))
    ));
    let Some(CoroutineState::Complete(Err(CodecError::Payload { source }))) = frame.as_mut().next()
    else {
        panic!("expected an incomplete payload");
    };
    assert_eq!(source.completed(), 2);
}

#[test]
fn multi_byte_length_and_wrong_type() {
    let mut input = b"\0\x40\x01x".as_slice();
    {
        let mut frame = pin!(input.sync_read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
        assert!(matches!(
            frame.as_mut().next(),
            Some(CoroutineState::Yielded(b"x"))
        ));
        assert!(matches!(
            frame.as_mut().next(),
            Some(CoroutineState::Complete(Ok(1)))
        ));
    }
    assert!(input.is_empty());
    let mut input = b"\x01\0".as_slice();
    let mut frame = pin!(input.sync_read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
    assert!(matches!(
        frame.as_mut().next(),
        Some(CoroutineState::Complete(Err(CodecError::WrongType {
            actual: 1
        })))
    ));
}

use nitori_call::PollCallExt as _;
use std::{
    pin::Pin,
    task::{Context, Poll, Waker},
};

pub(super) struct Output {
    bytes: Vec<u8>,
    waited: bool,
}
impl nitori_io::Write for Output {
    type Error = std::convert::Infallible;
    fn poll_write<'visit, Input: bytes::Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut Input,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let mut this = host.get_mut().0.as_mut();
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
#[test]
fn named_write_preserves_partial_progress_across_pending() {
    let mut output = Output {
        bytes: Vec::new(),
        waited: false,
    };
    let mut operation = pin!(nitori_io::calls::WriteAll::new(bytes::Bytes::from_static(
        b"abc"
    )));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut output), &mut cx)
            .is_pending()
    );
    assert!(output.bytes.is_empty());
    assert!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut output), &mut cx)
            .is_pending()
    );
    assert_eq!(output.bytes, b"ab");
    let Poll::Ready(CoroutineState::Complete(returned)) = operation
        .as_mut()
        .poll_receiver(Pin::new(&mut output), &mut cx)
    else {
        panic!("expected completion")
    };
    assert_eq!(returned.result.unwrap(), 3);
    assert!(returned.input.is_empty());
    assert_eq!(output.bytes, b"abc");
}
nitori_call::family_receiver!(impl [] for Output);

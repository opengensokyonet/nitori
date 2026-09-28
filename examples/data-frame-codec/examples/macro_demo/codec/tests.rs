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
    let Some(CoroutineState::Complete(Err(FrameError::Payload { source }))) = frame.as_mut().next()
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
        Some(CoroutineState::Complete(Err(FrameError::WrongType {
            actual: 1
        })))
    ));
}

#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

use bytes::{Buf, Bytes};
use nitori_call::PollCallExt as _;
use nitori_call::call;
use nitori_data_frame_codec_example::current_codec::TargetWriteExt as _;
use nitori_data_frame_codec_example::current_codec::{CodecError, WriteSink};
use std::{
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

struct Output {
    bytes: Vec<u8>,
    waited: bool,
}

impl<Input: Buf + ?Sized> WriteSink<&mut Input> for Output {
    fn poll_write<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        input: &mut &mut Input,
        cx: &mut Context<'_>,
    ) -> Poll<Result<usize, CodecError>>
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

#[call]
async fn write_local_buffer(
    io: nitori_call::Target<Output>,
) -> Result<(Bytes, usize, Bytes, usize), CodecError> {
    let mut buf = Bytes::from_static(b"abc");
    let returned = io.write(&mut buf).await;
    let first_count = returned.result?;
    let tail = buf.clone();
    let returned = io.write(&mut buf).await;
    let second_count = returned.result?;
    Ok((buf, first_count, tail, second_count))
}

#[test]
fn named_call_reborrows_local_buf_across_pending() {
    let mut output = Output {
        bytes: Vec::new(),
        waited: false,
    };
    let mut operation = pin!(write_local_buffer());
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
    let Poll::Ready(CoroutineState::Complete(Ok((buf, first_count, tail, second_count)))) =
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut output), &mut cx)
    else {
        panic!("expected completion")
    };
    assert_eq!(first_count, 2);
    assert_eq!(tail.as_ref(), b"c");
    assert_eq!(second_count, 1);
    assert!(buf.is_empty());
    assert_eq!(output.bytes, b"abc");
}

nitori_call::family_receiver!(impl [] for Output);

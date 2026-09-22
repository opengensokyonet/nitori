#![feature(coroutine_trait)]
use bytes::Bytes;
use nitori_call::PollCallExt as _;
use nitori_data_frame_codec_example::{
    current_codec::read_data_frame,
    current_codec::{CodecError, ReadSource},
};
use std::{
    num::NonZeroUsize,
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

struct Input(Bytes);
impl ReadSource for Input {
    fn poll_read<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        maximum: NonZeroUsize,
        _: &mut Context<'_>,
    ) -> Poll<Result<Option<Bytes>, CodecError>>
    where
        Self: 'visit,
    {
        let mut this = host.get_mut().0.as_mut();
        let count = maximum.get().min(this.0.len());
        Poll::Ready(Ok(if count == 0 {
            None
        } else {
            Some(this.0.split_to(count))
        }))
    }
}
fn main() {
    let mut input = Input(Bytes::from_static(b"\0\x05hello\0\0"));
    let mut call = pin!(read_data_frame::<Input>(NonZeroUsize::new(2).unwrap()));
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        match call.as_mut().poll_receiver(Pin::new(&mut input), &mut cx) {
            Poll::Ready(CoroutineState::Yielded(bytes)) => println!("macro chunk: {bytes:?}"),
            Poll::Ready(CoroutineState::Complete(result)) => {
                println!("macro complete: {result:?}; unconsumed: {:?}", input.0);
                break;
            }
            Poll::Pending => {
                panic!("demo source is fully ready and the operation has no scheduling budget")
            }
        }
    }
}

nitori_call::family_receiver!(impl [] for Input);

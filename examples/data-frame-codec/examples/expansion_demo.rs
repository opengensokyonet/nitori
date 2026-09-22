//! Source counterpart of expanded/expansion_demo.rs; both operations do the same work.
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use bytes::Bytes;
use nitori_call::CallOn;
use nitori_call::{PollCallExt as _, TargetExt as _};
use nitori_call::{call, call_closure};
use nitori_data_frame_codec_example::{
    current_codec::TargetReadVarintExt,
    current_codec::{CodecError, ReadSource},
};
use std::{
    future::ready,
    num::NonZeroUsize,
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

#[call(yields = u64)]
async fn read_pair<T: ReadSource>(io: nitori_call::Target<T>) -> Result<u64, CodecError> {
    let first = io.read_varint().await?;
    let first = io.with(|_| first).await;
    let offset = ready(0u64).await;
    yield first;
    Ok(first + io.read_varint().await? + offset)
}

struct Source(Bytes);
impl ReadSource for Source {
    fn poll_read<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        maximum: NonZeroUsize,
        _: &mut Context<'_>,
    ) -> Poll<Result<Option<Bytes>, CodecError>>
    where
        Self: 'visit,
    {
        let mut this = host.get_mut().0.as_mut();
        let count = this.0.len().min(maximum.get());
        Poll::Ready(Ok(if count == 0 {
            None
        } else {
            Some(this.0.split_to(count))
        }))
    }
}

fn check<Operation>(operation: Operation)
where
    Operation: CallOn<Source, Yield = u64, Return = Result<u64, CodecError>>,
{
    let mut operation = pin!(operation);
    let mut source = Source(Bytes::from_static(b"\x01\x02"));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut source), &mut cx),
        Poll::Ready(CoroutineState::Yielded(1))
    ));
    assert_eq!(source.0.as_ref(), b"\x02");
    assert!(matches!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut source), &mut cx),
        Poll::Ready(CoroutineState::Complete(Ok(3)))
    ));
    assert!(source.0.is_empty());
}
fn main() {
    check(read_pair());
    check(call_closure!(
        |io: nitori_call::Target<Source>| -> Result<u64, CodecError> {
            let first = io.read_varint().await?;
            let first = io.with(|_| first).await;
            let offset = ready(0u64).await;
            yield first;
            Ok(first + io.read_varint().await? + offset)
        }
    ));
}

nitori_call::family_receiver!(impl [] for Source);

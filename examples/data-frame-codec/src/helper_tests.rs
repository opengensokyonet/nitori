use crate::current_codec::{
    CodecError, ReadDataFrameExt, ReadSource, ReadSourceExt as _, ReadVarintExt,
};
use bytes::Bytes;
use nitori_call::call;
use nitori_call::{CallOn, Stream};
use nitori_call::{PollCallExt as _, ReceiverExt as _};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    marker::PhantomPinned,
    num::NonZeroUsize,
    ops::CoroutineState,
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
};

struct Memory {
    bytes: Bytes,
    wait: bool,
    polls: Rc<Cell<usize>>,
}
impl Memory {
    fn new(bytes: &'static [u8], wait: bool) -> Self {
        Self {
            bytes: Bytes::from_static(bytes),
            wait,
            polls: Rc::new(Cell::new(0)),
        }
    }
}
impl ReadSource for Memory {
    fn poll_read<'visit>(
        host: Pin<&mut Self::Host<'visit>>,
        maximum: NonZeroUsize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Bytes>, CodecError>>
    where
        Self: 'visit,
    {
        let mut this = host.get_mut().0.as_mut();
        this.polls.set(this.polls.get() + 1);
        if this.wait {
            this.wait = false;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let count = this.bytes.len().min(maximum.get());
        Poll::Ready(Ok(if count == 0 {
            None
        } else {
            Some(this.bytes.split_to(count))
        }))
    }
}
#[test]
fn unpin_helper_is_a_future_and_releases_target_on_drop() {
    let mut source = Memory::new(b"\x40\x41tail", true);
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut future = pin!(source.read_varint_unpin());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(65))));
    }
    assert_eq!(source.bytes.as_ref(), b"tail");
}
struct PinnedMemory {
    inner: RefCell<Memory>,
    _pin: PhantomPinned,
}
impl ReadSource for PinnedMemory {
    fn poll_read<'visit>(
        host: Pin<&mut Self::Host<'visit>>,
        maximum: NonZeroUsize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Bytes>, CodecError>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        Pin::new(&mut *this.inner.borrow_mut()).poll_read(maximum, cx)
    }
}
#[test]
fn pinned_helper_supports_non_unpin_and_dyn_targets() {
    let mut source = pin!(PinnedMemory {
        inner: RefCell::new(Memory::new(b"\x01\x02", false)),
        _pin: PhantomPinned
    });
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut future = pin!(source.as_mut().read_varint());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(1))));
    }
    let mut view = nitori_call::Host::view(source.as_mut());
    let erased = Pin::new(&mut view);
    let mut future = pin!(erased.read_varint());
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(2))));
}
#[test]
fn streaming_helper_delivers_completion_once_and_preserves_backpressure() {
    let mut source = Memory::new(b"\0\x03abc\0\0", false);
    let polls = source.polls.clone();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut stream = pin!(source.read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
        let Poll::Ready(Some(CoroutineState::Yielded(chunk))) = stream.as_mut().poll_next(&mut cx)
        else {
            panic!("chunk")
        };
        assert_eq!(chunk.bytes.as_ref(), b"ab");
        assert_eq!(polls.get(), 3);
        {
            let mut next = pin!(stream.as_mut().next());
            let Poll::Ready(Some(CoroutineState::Yielded(chunk))) = next.as_mut().poll(&mut cx)
            else {
                panic!("chunk")
            };
            assert_eq!(chunk.bytes.as_ref(), b"c");
        }
        assert_eq!(polls.get(), 4);
        assert!(matches!(
            stream.as_mut().poll_next(&mut cx),
            Poll::Ready(Some(CoroutineState::Complete(Ok(3))))
        ));
        assert!(matches!(
            stream.as_mut().poll_next(&mut cx),
            Poll::Ready(None)
        ));
        assert!(matches!(
            stream.as_mut().poll_next(&mut cx),
            Poll::Ready(None)
        ));
        assert_eq!(polls.get(), 4);
    }
    assert_eq!(source.bytes.as_ref(), b"\0\0");
}
#[test]
fn awaiting_yielding_call_discards_items_and_preserves_pending() {
    let mut source = Memory::new(b"\0\x03abc\0\0", true);
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut future = pin!(source.read_data_frame_unpin(NonZeroUsize::new(1).unwrap()));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(3))));
    }
    assert_eq!(source.bytes.as_ref(), b"\0\0");
}
#[test]
fn cancelling_next_preserves_stream_but_dropping_stream_releases_target() {
    let mut source = Memory::new(b"\0\x03abc", true);
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut stream = pin!(source.read_data_frame_unpin(NonZeroUsize::new(1).unwrap()));
        {
            let mut next = pin!(stream.as_mut().next());
            assert!(next.as_mut().poll(&mut cx).is_pending());
        }
        assert!(matches!(
            stream.as_mut().poll_next(&mut cx),
            Poll::Ready(Some(CoroutineState::Yielded(_)))
        ));
    }
    assert_eq!(source.bytes.as_ref(), b"bc");
}
struct DropGuard(Rc<Cell<usize>>);
impl Drop for DropGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1)
    }
}
#[call]
async fn wait_forever(io: nitori_call::Receiver<nitori_call::Direct<usize>>, guard: DropGuard) {
    io.with(|__access| {
        let mut target = __access.into_pin().get_mut().0.as_mut();
        *target += 1
    })
    .await;
    std::future::pending::<()>().await;
    drop(guard);
}
#[test]
fn helper_cancellation_drops_operation_without_revisiting_target() {
    let drops = Rc::new(Cell::new(0));
    let mut target = 0usize;
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut future = pin!(target.wait_forever_unpin(DropGuard(drops.clone())));
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(drops.get(), 1);
    assert_eq!(target, 1);
}
trait Fallible {
    type Error;
    fn get(&self) -> Result<usize, Self::Error>;
}
impl Fallible for usize {
    type Error = ();
    fn get(&self) -> Result<usize, ()> {
        Ok(*self)
    }
}
#[call(yields = &'data str)]
async fn generic_helper<'data, T: Fallible + ?Sized, const EXTRA: usize>(
    io: nitori_call::Receiver<nitori_call::Direct<T>>,
    text: &'data str,
) -> Result<usize, T::Error> {
    yield text;
    io.with(|__access| {
        let target = __access.into_pin().get_mut().0.as_mut();
        target.get()
    })
    .await
    .map(|value| value + EXTRA)
}
#[test]
fn helper_preserves_lifetimes_const_arguments_and_target_errors() {
    let text = String::from("borrowed");
    let mut target = 4usize;
    let mut stream = pin!(<usize as GenericHelperExt<usize, 3>>::generic_helper_unpin(
        &mut target,
        &text
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        stream.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(CoroutineState::Yielded("borrowed")))
    ));
    assert!(matches!(
        stream.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(CoroutineState::Complete(Ok(7))))
    ));
}
// The new contract permits target-dependent signatures on one operation type.
struct Identity;
impl<T: Copy> CallOn<nitori_call::Direct<T>> for Identity {
    type Yield = std::convert::Infallible;
    type Return = T;
    fn poll_call<'v>(
        self: Pin<&mut Self>,
        target: Pin<&mut nitori_call::DirectView<'v, T>>,
        _: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, T>>
    where
        nitori_call::Direct<T>: 'v,
    {
        Poll::Ready(CoroutineState::Complete(*target.get_mut().0))
    }
}
#[test]
fn one_operation_type_has_a_signature_for_each_target() {
    let mut cx = Context::from_waker(Waker::noop());
    let mut number = 7u32;
    let mut flag = true;
    assert_eq!(
        Pin::new(&mut Identity).poll_host(Pin::new(&mut number), &mut cx),
        Poll::Ready(CoroutineState::Complete(7))
    );
    assert_eq!(
        Pin::new(&mut Identity).poll_host(Pin::new(&mut flag), &mut cx),
        Poll::Ready(CoroutineState::Complete(true))
    );
}

impl crate::current_codec::WriteSink<Bytes> for nitori_call::Direct<Vec<u8>> {
    fn poll_write<'visit>(
        host: Pin<&mut Self::Host<'visit>>,
        input: &mut Bytes,
        _: &mut Context<'_>,
    ) -> Poll<Result<usize, CodecError>>
    where
        Self: 'visit,
    {
        let mut this = host.get_mut().0.as_mut();
        let chunk = input.split_to(input.len().min(1));
        this.extend_from_slice(&chunk);
        Poll::Ready(Ok(chunk.len()))
    }
}
#[test]
fn write_helper_infers_input_generic_and_preserves_owned_result() {
    let mut target = Vec::new();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut future = pin!(write_through_trait_only(
            &mut target,
            Bytes::from_static(b"abc")
        ));
        let Poll::Ready(returned) = future.as_mut().poll(&mut cx) else {
            panic!("completion")
        };
        assert_eq!(returned.result.unwrap(), 3);
        assert!(returned.input.is_empty());
    }
    assert_eq!(target, b"abc");
}

async fn through_trait_only<F: ReadSource, T: ReadVarintExt<F> + ?Sized>(
    target: Pin<&mut T>,
) -> Result<u64, CodecError> {
    target.read_varint().await
}
#[test]
fn caller_can_require_only_the_helper_trait() {
    let mut source = Memory::new(b"\x03", false);
    let mut future = pin!(through_trait_only(Pin::new(&mut source)));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(3))));
}

#[test]
fn no_yield_operation_still_exposes_completion_as_a_stream() {
    let mut source = Memory::new(b"\x05", false);
    let mut stream = pin!(source.read_varint_unpin());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        stream.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(CoroutineState::Complete(Ok(5))))
    ));
    assert!(matches!(
        stream.as_mut().poll_next(&mut cx),
        Poll::Ready(None)
    ));
}
#[test]
fn future_continues_after_stream_delivery_without_replaying_items() {
    let mut source = Memory::new(b"\0\x03abc\0\0", false);
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut call = pin!(source.read_data_frame_unpin(NonZeroUsize::new(1).unwrap()));
        let Poll::Ready(Some(CoroutineState::Yielded(chunk))) = call.as_mut().poll_next(&mut cx)
        else {
            panic!("chunk")
        };
        assert_eq!(chunk.bytes.as_ref(), b"a");
        assert!(matches!(call.as_mut().poll(&mut cx), Poll::Ready(Ok(3))));
        assert!(matches!(
            call.as_mut().poll_next(&mut cx),
            Poll::Ready(None)
        ));
    }
    assert_eq!(source.bytes.as_ref(), b"\0\0");
}

async fn write_through_trait_only<
    F: crate::current_codec::WriteSink<Bytes>,
    T: crate::current_codec::WriteAllExt<F, Bytes> + ?Sized + Unpin,
>(
    target: &mut T,
    input: Bytes,
) -> crate::current_codec::WriteReturn<Bytes> {
    target.write_all_unpin(input).await
}

nitori_call::family_host!(impl [] for Memory);

nitori_call::family_host!(impl [] for PinnedMemory);

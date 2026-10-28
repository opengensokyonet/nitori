use crate::current_codec::{ReadDataFrameHostExt as _, ReadVarintHostExt};
use bytes::{BufMut, Bytes};
use futures_core::Stream;
use nitori_call::{
    CallExecutor, Direct, Emitter, Operation, ReceiverAccess, ReceiverFamily, Start, call,
};
use nitori_io::calls::{ReadBeError, WriteAllError, WriteAllHostExt as _};
use nitori_io::{PollReadExt as _, Read, ReadChunk};
use std::convert::Infallible;
use std::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    marker::PhantomPinned,
    num::NonZeroUsize,
    ops::ControlFlow,
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
};

/// Complete a future that does not wait.
fn run<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("memory input does not wait"),
    }
}

/// The next event of a stream.
fn next<S: Stream + ?Sized>(mut stream: Pin<&mut S>) -> impl Future<Output = Option<S::Item>> + '_ {
    poll_fn(move |cx| stream.as_mut().poll_next(cx))
}

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
impl Read for Memory {
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
impl ReadChunk for Memory {
    type Chunk = Bytes;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Bytes>, Self::Error>>
    where
        Self: 'visit,
    {
        let mut this = host.get_pin_mut();
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
impl Read for PinnedMemory {
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
impl ReadChunk for PinnedMemory {
    type Chunk = Bytes;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Bytes>, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        Pin::new(&mut *this.inner.borrow_mut()).poll_read_chunk(cx, maximum)
    }
}
#[test]
fn pinned_helper_supports_non_unpin_targets() {
    let mut source = pin!(PinnedMemory {
        inner: RefCell::new(Memory::new(b"\x01\x02", false)),
        _pin: PhantomPinned
    });
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut future = pin!(source.as_mut().read_varint());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(1))));
    }
    let mut future = pin!(source.as_mut().read_varint());
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(2))));
}
#[test]
fn streaming_helper_delivers_completion_once_and_preserves_backpressure() {
    let mut source = Memory::new(b"\0\x03abc\0\0", false);
    let polls = source.polls.clone();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut stream = pin!(source.read_data_frame_unpin(NonZeroUsize::new(2).unwrap()));
        let Poll::Ready(Some(ControlFlow::Continue(chunk))) = stream.as_mut().poll_next(&mut cx)
        else {
            panic!("chunk")
        };
        assert_eq!(chunk.as_ref(), b"ab");
        assert_eq!(polls.get(), 3);
        {
            let mut next = pin!(next(stream.as_mut()));
            let Poll::Ready(Some(ControlFlow::Continue(chunk))) = next.as_mut().poll(&mut cx)
            else {
                panic!("chunk")
            };
            assert_eq!(chunk.as_ref(), b"c");
        }
        assert_eq!(polls.get(), 4);
        assert!(matches!(
            stream.as_mut().poll_next(&mut cx),
            Poll::Ready(Some(ControlFlow::Break(Ok(3))))
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
            let mut next = pin!(next(stream.as_mut()));
            assert!(next.as_mut().poll(&mut cx).is_pending());
        }
        assert!(matches!(
            stream.as_mut().poll_next(&mut cx),
            Poll::Ready(Some(ControlFlow::Continue(_)))
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
/// A family whose view holds a number.
trait Counter: ReceiverFamily {
    fn bump<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> usize
    where
        Self: 'v;
}
impl Counter for Direct<usize> {
    fn bump<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> usize
    where
        Self: 'v,
    {
        let mut target = view.get_pin_mut();
        *target += 1;
        *target
    }
}
async fn bump<A: ReceiverAccess<Family: Counter>>(io: &mut A) -> usize {
    poll_fn(|cx| {
        io.poll_access(cx, |access, _| {
            Poll::Ready(A::Family::bump(access.into_pin()))
        })
    })
    .await
}
#[call]
async fn wait_forever<A: ReceiverAccess<Family: Counter>>(io: &mut A, guard: DropGuard) {
    bump(io).await;
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
/// A family whose view can produce a number or fail.
trait FallibleFamily: ReceiverFamily {
    type Error;
    fn value<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> Result<usize, Self::Error>
    where
        Self: 'v;
}
impl<T: Fallible + ?Sized> FallibleFamily for Direct<T> {
    type Error = T::Error;
    fn value<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> Result<usize, T::Error>
    where
        Self: 'v,
    {
        view.get_mut().0.as_ref().get_ref().get()
    }
}
#[call(yields = &'data str)]
async fn generic_helper<'data, A: ReceiverAccess<Family: FallibleFamily>, const EXTRA: usize>(
    io: &mut A,
    text: &'data str,
) -> Result<usize, <A::Family as FallibleFamily>::Error> {
    yield text;
    poll_fn(|cx| {
        io.poll_access(cx, |access, _| {
            Poll::Ready(A::Family::value(access.into_pin()))
        })
    })
    .await
    .map(|value| value + EXTRA)
}
#[test]
fn helper_preserves_lifetimes_const_arguments_and_target_errors() {
    let text = String::from("borrowed");
    let mut target = 4usize;
    let mut stream = pin!(<usize as GenericHelperHostExt>::generic_helper_unpin::<3>(
        &mut target,
        &text
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        stream.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(ControlFlow::Continue("borrowed")))
    ));
    assert!(matches!(
        stream.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(ControlFlow::Break(Ok(7))))
    ));
}
// One operation type has an output for each family.
struct Identity;
impl<T: Copy> Operation<Direct<T>> for Identity {
    type Output = T;
    type Yield = Infallible;
}
impl<'s, T: Copy, A: ReceiverAccess<Family = Direct<T>>> Start<'s, A> for Identity {
    type Fut = impl Future<Output = T>;
    fn start(self, mut io: A, _: Emitter<'s, Infallible>) -> Self::Fut {
        async move {
            poll_fn(|cx| {
                io.poll_access(cx, |access, _| Poll::Ready(*access.into_pin().get_mut().0))
            })
            .await
        }
    }
}
#[test]
fn one_operation_type_has_a_signature_for_each_target() {
    let mut number = 7u32;
    let mut flag = true;
    assert_eq!(
        run(CallExecutor::execute(Pin::new(&mut number), Identity)),
        7
    );
    assert!(run(CallExecutor::execute(Pin::new(&mut flag), Identity)));
}

#[test]
fn write_helper_infers_input_generic() {
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
        returned.unwrap();
    }
    assert_eq!(target, b"abc");
}

async fn through_trait_only<T: ReadVarintHostExt + ?Sized>(
    target: Pin<&mut T>,
) -> Result<u64, ReadBeError<<T::Family as Read>::Error>>
where
    T::Family: Read,
{
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
        Poll::Ready(Some(ControlFlow::Break(Ok(5))))
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
        let Poll::Ready(Some(ControlFlow::Continue(chunk))) = call.as_mut().poll_next(&mut cx)
        else {
            panic!("chunk")
        };
        assert_eq!(chunk.as_ref(), b"a");
        assert!(matches!(call.as_mut().poll(&mut cx), Poll::Ready(Ok(3))));
        assert!(matches!(
            call.as_mut().poll_next(&mut cx),
            Poll::Ready(None)
        ));
    }
    assert_eq!(source.bytes.as_ref(), b"\0\0");
}

async fn write_through_trait_only<
    F: nitori_io::Write,
    T: CallExecutor<Family = F> + ?Sized + Unpin,
>(
    target: &mut T,
    input: Bytes,
) -> Result<(), WriteAllError<F::Error>> {
    target.write_all_unpin(input).await
}

nitori_call::family_receiver!(impl [] for Memory);

nitori_call::family_receiver!(impl [] for PinnedMemory);

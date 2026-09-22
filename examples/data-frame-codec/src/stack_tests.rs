use crate::current_codec::{CodecError, ReadSource};
use crate::current_codec::{TargetReadAtMostExt, TargetReadVarintExt};
use bytes::Bytes;
use nitori_call::CallOn;
use nitori_call::call_closure;
use nitori_call::{PollCallExt as _, TargetExt as _};
use std::num::NonZeroUsize;
use std::{
    cell::Cell,
    convert::Infallible,
    future::{Future, poll_fn, ready},
    marker::PhantomPinned,
    ops::CoroutineState,
    pin::{Pin, pin},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

struct Target {
    value: Cell<usize>,
    _pin: PhantomPinned,
}
impl Target {
    fn value(&self) -> usize {
        self.value.get()
    }
}
fn target() -> Target {
    Target {
        value: Cell::new(0),
        _pin: PhantomPinned,
    }
}
struct WriteLocal<'data> {
    text: &'data mut String,
    waited: bool,
}
impl<'data> WriteLocal<'data> {
    fn new(text: &'data mut String) -> Self {
        Self {
            text,
            waited: false,
        }
    }
}

impl CallOn<nitori_call::Direct<Target>> for WriteLocal<'_> {
    type Yield = Infallible;
    type Return = usize;
    fn poll_call<'v>(
        self: Pin<&mut Self>,
        view: Pin<&mut dyn nitori_call::ReceiverScope<'v, Family = nitori_call::Direct<Target>>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Infallible, usize>>
    where
        nitori_call::Direct<Target>: 'v,
    {
        let view = std::task::ready!(view.poll_view(cx));

        let target = view.get_mut().0.as_mut();
        let this = self.get_mut();
        if !this.waited {
            this.waited = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.text.push('!');
        target.value.set(this.text.len());
        Poll::Ready(CoroutineState::Complete(this.text.len()))
    }
}
#[test]
fn locals_and_child_borrows_survive_pending_then_emit() {
    let mut operation = pin!(call_closure!(|io: nitori_call::Target<
        nitori_call::Direct<Target>,
    >| {
        let mut text = String::from("local");
        let mut observed = 0;
        io.with(|__access| {
            let target = __access.into_pin().get_mut().0.as_mut();
            observed = target.value() + text.len()
        })
        .await;
        let written = io.operation(WriteLocal::new(&mut text)).await;
        let mut waited = false;
        let length = poll_fn(|cx| {
            if waited {
                Poll::Ready(text.len())
            } else {
                waited = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
        yield (observed, written, length);
        (
            text,
            io.with(|access| access.into_pin().as_ref().get_ref().0.value())
                .await,
        )
    }));
    let mut target = pin!(target());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        operation
            .as_mut()
            .poll_receiver(target.as_mut(), &mut cx)
            .is_pending()
    );
    target.value.set(100);
    assert!(
        operation
            .as_mut()
            .poll_receiver(target.as_mut(), &mut cx)
            .is_pending()
    );
    assert!(matches!(
        operation.as_mut().poll_receiver(target.as_mut(), &mut cx),
        Poll::Ready(CoroutineState::Yielded((5, 6, 6)))
    ));
    assert!(
        matches!(operation.as_mut().poll_receiver(target.as_mut(),&mut cx),Poll::Ready(CoroutineState::Complete((text,6))) if text=="local!")
    );
}
#[test]
fn send_sync_are_derived_from_state_not_target() {
    fn require_both(_: &(impl Send + Sync)) {}
    // Rc<Cell<_>> is neither Send nor Sync, but it is not captured by the call.
    let operation = call_closure!(|io: nitori_call::Target<
        nitori_call::Direct<Rc<Cell<usize>>>,
    >| {
        yield io
            .with(|__access| {
                let target = __access.into_pin().get_mut().0.as_mut();
                target.get()
            })
            .await;
        io.with(|__access| {
            let target = __access.into_pin().get_mut().0.as_mut();
            target.get()
        })
        .await
    });
    require_both(&operation);
    let mut operation = Box::pin(operation);
    let mut first = Rc::new(Cell::new(7));
    assert!(matches!(
        operation.as_mut().poll_receiver(
            Pin::new(&mut first),
            &mut Context::from_waker(Waker::noop())
        ),
        Poll::Ready(CoroutineState::Yielded(7))
    ));
    // Only the pinned state owner is moved; neither target nor the first cx follows it.
    let result = std::thread::spawn(move || {
        let mut second = Rc::new(Cell::new(9));
        operation.as_mut().poll_receiver(
            Pin::new(&mut second),
            &mut Context::from_waker(Waker::noop()),
        )
    })
    .join()
    .unwrap();
    assert!(matches!(result, Poll::Ready(CoroutineState::Complete(9))));
    assert_eq!(first.get(), 7);
}
struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
#[test]
fn new_context_after_pending_on_another_thread() {
    let first = Arc::new(WakeCount(AtomicUsize::new(0)));
    let second = Arc::new(WakeCount(AtomicUsize::new(0)));
    let mut operation = Box::pin(call_closure!(|io: nitori_call::Target<
        nitori_call::Direct<usize>,
    >| {
        let mut waited = false;
        poll_fn(|cx| {
            cx.waker().wake_by_ref();
            if waited {
                Poll::Ready(())
            } else {
                waited = true;
                Poll::Pending
            }
        })
        .await;
        io.with(|__access| {
            let target = __access.into_pin().get_mut().0.as_mut();
            *target
        })
        .await
    }));
    let mut target = 1usize;
    let waker = Waker::from(first.clone());
    assert!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut target), &mut Context::from_waker(&waker))
            .is_pending()
    );
    let next = second.clone();
    assert!(matches!(
        std::thread::spawn(move || {
            let mut target = 2usize;
            let waker = Waker::from(next);
            operation
                .as_mut()
                .poll_receiver(Pin::new(&mut target), &mut Context::from_waker(&waker))
        })
        .join()
        .unwrap(),
        Poll::Ready(CoroutineState::Complete(2))
    ));
    assert_eq!(first.0.load(Ordering::SeqCst), 1);
    assert_eq!(second.0.load(Ordering::SeqCst), 1);
}
struct DropProbe(Arc<AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
impl Future for DropProbe {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
}
#[test]
fn cancellation_on_another_thread_drops_only_captured_state() {
    let drops = Arc::new(AtomicUsize::new(0));
    let capture = drops.clone();
    let mut operation = Box::pin(call_closure!(move |io: nitori_call::Target<
        nitori_call::Direct<usize>,
    >| {
        io.with(|__access| {
            let mut target = __access.into_pin().get_mut().0.as_mut();
            *target = 4
        })
        .await;
        DropProbe(capture).await;
        io.with(|__access| {
            let mut target = __access.into_pin().get_mut().0.as_mut();
            *target = 8
        })
        .await;
    }));
    let mut target = 0usize;
    assert!(
        operation
            .as_mut()
            .poll_receiver(
                Pin::new(&mut target),
                &mut Context::from_waker(Waker::noop())
            )
            .is_pending()
    );
    std::thread::spawn(move || drop(operation)).join().unwrap();
    assert_eq!(target, 4);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[test]
fn nested_call_closure_and_real_with_alias_need_no_shared_slot() {
    let inner = call_closure!(|io: nitori_call::Target<nitori_call::Direct<usize>>| {
        ready(()).await;
        io.with(|__access| {
            let io = __access.into_pin().get_mut().0.as_mut();
            {
                let mut alias = io;
                *alias += 1;
            }
        })
        .await;
        io.with(|__access| {
            let target = __access.into_pin().get_mut().0.as_mut();
            *target
        })
        .await
    });
    let mut operation = pin!(call_closure!(move |io: nitori_call::Target<
        nitori_call::Direct<usize>,
    >| {
        let value = io.operation(inner).await;
        yield value;
        io.with(|__access| {
            let mut target = __access.into_pin().get_mut().0.as_mut();
            *target += 1
        })
        .await;
        value + 1
    }));
    let mut target = 0usize;
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut target), &mut cx),
        Poll::Ready(CoroutineState::Yielded(1))
    ));
    assert_eq!(target, 1);
    assert!(matches!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut target), &mut cx),
        Poll::Ready(CoroutineState::Complete(2))
    ));
    assert_eq!(target, 2);
}
#[test]
fn hidden_environment_cannot_be_shadowed_by_spelling() {
    let mut operation = pin!(call_closure!(|io: nitori_call::Target<
        nitori_call::Direct<usize>,
    >| {
        let __stack_environment = 41;
        let __stack_child = 1;
        let __call_closureback = 2;
        io.with(|__access| {
            let mut target = __access.into_pin().get_mut().0.as_mut();
            *target = __stack_child + __call_closureback
        })
        .await;
        yield __stack_environment;
        io.with(|__access| {
            let target = __access.into_pin().get_mut().0.as_mut();
            *target
        })
        .await
    }));
    let mut target = 0usize;
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut target), &mut cx),
        Poll::Ready(CoroutineState::Yielded(41))
    ));
    assert!(matches!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut target), &mut cx),
        Poll::Ready(CoroutineState::Complete(3))
    ));
}

struct Source {
    bytes: Bytes,
    reads: usize,
}
impl Source {
    fn remaining(&self) -> usize {
        self.bytes.len()
    }
}
impl ReadSource for Source {
    fn poll_read<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        maximum: NonZeroUsize,
        _: &mut Context<'_>,
    ) -> Poll<Result<Option<Bytes>, CodecError>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        let this = this.get_mut();
        this.reads += 1;
        if this.bytes.is_empty() {
            return Poll::Ready(Ok(None));
        }
        let count = this.bytes.len().min(maximum.get()).min(2);
        Poll::Ready(Ok(Some(this.bytes.split_to(count))))
    }
}
#[test]
fn data_frame_yields_without_reading_ahead_and_keeps_next_frame() {
    let mut operation = pin!(call_closure!(|io: nitori_call::Target<Source>| {
        let kind = io.read_varint().await?;
        if kind != 0 {
            return Err(CodecError::WrongType);
        }
        let length = io.read_varint().await?;
        let mut remaining = length as usize;
        while remaining != 0 {
            let part = io
                .read_at_most(NonZeroUsize::new(remaining.min(3)).unwrap())
                .await?
                .ok_or(CodecError::Eof)?;
            remaining -= part.len();
            yield part;
        }
        Ok(length)
    }));
    let mut target = Source {
        bytes: Bytes::from_static(b"\x00\x05hello\x00\x00"),
        reads: 0,
    };
    let mut cx = Context::from_waker(Waker::noop());
    for (expected, remaining, reads) in [
        (b"he".as_slice(), 5, 3),
        (b"ll".as_slice(), 3, 4),
        (b"o".as_slice(), 2, 5),
    ] {
        let Poll::Ready(CoroutineState::Yielded(part)) = operation
            .as_mut()
            .poll_receiver(Pin::new(&mut target), &mut cx)
        else {
            panic!("expected chunk")
        };
        assert_eq!(part.as_ref(), expected);
        assert_eq!((target.remaining(), target.reads), (remaining, reads));
    }
    assert!(matches!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut target), &mut cx),
        Poll::Ready(CoroutineState::Complete(Ok(5)))
    ));
    assert_eq!(target.reads, 5);
}

#[test]
fn panic_drops_user_state_and_poisoning_prevents_another_resume() {
    fn fail() -> ! {
        panic!("test callback panic")
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let capture = drops.clone();
    let mut operation = Box::pin(call_closure!(move |io: nitori_call::Target<
        nitori_call::Direct<usize>,
    >| {
        let _guard = DropProbe(capture);
        io.with(|__access| {
            let mut target = __access.into_pin().get_mut().0.as_mut();
            {
                *target = 3;
                fail();
            }
        })
        .await;
    }));
    let mut target = 0usize;
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation
            .as_mut()
            .poll_receiver(Pin::new(&mut target), &mut cx)))
        .is_err()
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    target = 5;
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation
            .as_mut()
            .poll_receiver(Pin::new(&mut target), &mut cx)))
        .is_err()
    );
    drop(operation);
    assert_eq!(target, 5);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn temporary_target_reborrows_allow_nested_synchronous_calls() {
    fn inner(mut target: Pin<&mut usize>) -> usize {
        let mut child = pin!(call_closure!(|io: nitori_call::Target<
            nitori_call::Direct<usize>,
        >| {
            io.with(|__access| {
                let mut target = __access.into_pin().get_mut().0.as_mut();
                {
                    *target += 1;
                    *target
                }
            })
            .await
        }));
        match child
            .as_mut()
            .poll_receiver(target.as_mut(), &mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(CoroutineState::Complete(value)) => value,
            _ => panic!("expected completion"),
        }
    }
    let mut outer = pin!(call_closure!(|io: nitori_call::Target<
        nitori_call::Direct<usize>,
    >| {
        io.with(|__access| {
            let mut target = __access.into_pin().get_mut().0.as_mut();
            {
                let value = inner(target.as_mut());
                *target += value;
                *target
            }
        })
        .await
    }));
    let mut target = 0usize;
    assert!(matches!(
        outer.as_mut().poll_receiver(
            Pin::new(&mut target),
            &mut Context::from_waker(Waker::noop())
        ),
        Poll::Ready(CoroutineState::Complete(2))
    ));
}

#[test]
fn non_static_dyn_target_needs_no_owned_access_storage() {
    trait Value {
        fn value(&self) -> usize;
    }
    struct Borrowed<'data>(&'data usize);
    impl Value for Borrowed<'_> {
        fn value(&self) -> usize {
            *self.0
        }
    }
    let number = 7;
    let mut target = pin!(Borrowed(&number));
    let mut operation = pin!(call_closure!(|io: nitori_call::Target<
        nitori_call::Direct<dyn Value + '_>,
    >| {
        yield io
            .with(|access| access.into_pin().as_ref().get_ref().0.value())
            .await;
        io.with(|access| access.into_pin().as_ref().get_ref().0.value())
            .await
            + 1
    }));
    let mut view = nitori_call::DirectView(target.as_mut() as Pin<&mut (dyn Value + '_)>);
    let mut target = Pin::new(&mut view);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        operation.as_mut().poll_receiver(target.as_mut(), &mut cx),
        Poll::Ready(CoroutineState::Yielded(7))
    ));
    assert!(matches!(
        operation.as_mut().poll_receiver(target.as_mut(), &mut cx),
        Poll::Ready(CoroutineState::Complete(8))
    ));
}

#[test]
fn callback_construction_can_suspend_without_retaining_the_old_environment() {
    let mut operation = pin!(call_closure!(|io: nitori_call::Target<
        nitori_call::Direct<usize>,
    >| {
        io.with({
            yield ();
            |access: nitori_call::Access<'_, '_, nitori_call::Direct<usize>>| {
                let mut target = access.into_pin().get_mut().0.as_mut();
                *target += 1;
                *target
            }
        })
        .await
    }));
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut first = 10usize;
        assert!(matches!(
            operation
                .as_mut()
                .poll_receiver(Pin::new(&mut first), &mut cx),
            Poll::Ready(CoroutineState::Yielded(()))
        ));
        assert_eq!(first, 10);
    }
    let mut second = 20usize;
    assert!(matches!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut second), &mut cx),
        Poll::Ready(CoroutineState::Complete(21))
    ));
    assert_eq!(second, 21);
}

#[nitori_call::call(yields = usize)]
async fn named_local<'data>(
    io: nitori_call::Target<nitori_call::Direct<Target>>,
    text: &'data mut String,
) -> usize {
    let first = io.operation(WriteLocal::new(text)).await;
    let unrelated = || 7;
    let second = ready(unrelated()).await;
    io.with({
        yield first;
        |access: nitori_call::Access<'_, '_, nitori_call::Direct<Target>>| {
            access.into_pin().get_mut().0.value.set(second)
        }
    })
    .await;
    io.with(|access| access.into_pin().as_ref().get_ref().0.value())
        .await
}

#[nitori_call::call]
async fn named_value<Target: ValueSource + ?Sized>(
    io: nitori_call::Target<nitori_call::Direct<Target>>,
    offset: usize,
) -> usize {
    io.with(|__access| {
        let target = __access.into_pin().get_mut().0.as_mut();
        target.value()
    })
    .await
        + ready(offset).await
}
trait ValueSource {
    fn value(&self) -> usize;
}
impl ValueSource for Target {
    fn value(&self) -> usize {
        self.value.get()
    }
}

#[nitori_call::call]
async fn named_parent<Target: ValueSource + ?Sized>(
    io: nitori_call::Target<nitori_call::Direct<Target>>,
) -> usize {
    io.named_value(2).await
}

#[test]
fn named_operation_preserves_pending_borrows_and_business_yields() {
    let mut text = String::from("named");
    let mut operation = pin!(named_local(&mut text));
    let mut target = pin!(target());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        operation
            .as_mut()
            .poll_receiver(target.as_mut(), &mut cx)
            .is_pending()
    );
    assert_eq!(
        operation.as_mut().poll_receiver(target.as_mut(), &mut cx),
        Poll::Ready(CoroutineState::Yielded(6))
    );
    assert_eq!(target.value(), 6);
    assert_eq!(
        operation.as_mut().poll_receiver(target.as_mut(), &mut cx),
        Poll::Ready(CoroutineState::Complete(7))
    );
}

#[test]
fn named_generic_composition_and_closure_return_annotation() {
    let mut operation = pin!(call_closure!(|io: nitori_call::Target<
        nitori_call::Direct<Target>,
    >|
     -> usize { io.named_parent().await }));
    let mut target = pin!(target());
    target.value.set(10);
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(
        operation.as_mut().poll_receiver(target.as_mut(), &mut cx),
        Poll::Ready(CoroutineState::Complete(12))
    );
    let mut named = pin!(named_parent::<dyn ValueSource>());
    let mut view = nitori_call::DirectView(target.as_mut() as Pin<&mut dyn ValueSource>);
    let erased = Pin::new(&mut view);
    assert_eq!(
        named.as_mut().poll_receiver(erased, &mut cx),
        Poll::Ready(CoroutineState::Complete(12))
    );
}

nitori_call::family_receiver!(impl [] for Source);

nitori_call::direct_receiver!(impl [] for Target);

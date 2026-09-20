#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
#![deny(unsafe_op_in_unsafe_fn)]
use sakuya_call::CallOn;
use sakuya_call::call_closure;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    convert::Infallible,
    future::{Future, poll_fn},
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

struct CountingAllocator;
thread_local! {
    static COUNTING: Cell<bool> = const {Cell::new(false)};
    static ALLOCATIONS: Cell<usize> = const {Cell::new(0)};
}
fn record() {
    let _ = COUNTING.try_with(|counting| {
        if counting.get() {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        }
    });
}
// SAFETY: this allocator transparently forwards all allocations and deallocations
// to System. Counters are thread-local, constant-initialized, and allocation-free.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
struct ReadByte;
impl ReadByte {
    fn new() -> Self {
        Self
    }
}

impl CallOn<u8> for ReadByte {
    type Yield = Infallible;
    type Return = u8;
    fn poll_call(
        self: Pin<&mut Self>,
        mut target: Pin<&mut u8>,
        _: &mut Context<'_>,
    ) -> Poll<CoroutineState<Infallible, u8>> {
        *target += 1;
        Poll::Ready(CoroutineState::Complete(*target))
    }
}
#[sakuya_call::call]
async fn named_byte(io: ::core::pin::Pin<&mut u8>) -> u8 {
    io.read_byte().await
}
#[test]
fn construction_ready_pending_emit_completion_and_drop_allocate_nothing() {
    let mut target = 0;
    let mut cx = Context::from_waker(Waker::noop());
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|flag| flag.set(true));
    let (emitted, waits, returned) = {
        let mut operation = pin!(call_closure!(|io: ::core::pin::Pin<&mut u8>| {
            let mut sum = 0usize;
            for _ in 0..4 {
                let mut waited = false;
                poll_fn(|cx| {
                    if waited {
                        Poll::Ready(())
                    } else {
                        waited = true;
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;
                let byte = io.named_byte().await;
                sum += usize::from(byte);
                yield byte;
            }
            sum
        }));
        let mut emitted = 0;
        let mut waits = 0;
        let returned = loop {
            match operation.as_mut().poll_call(Pin::new(&mut target), &mut cx) {
                Poll::Pending => waits += 1,
                Poll::Ready(CoroutineState::Yielded(_)) => emitted += 1,
                Poll::Ready(CoroutineState::Complete(value)) => break value,
            }
        };
        (emitted, waits, returned)
    };
    COUNTING.with(|flag| flag.set(false));
    assert_eq!((emitted, waits, returned, target), (4, 4, 10, 4));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
}

#[test]
fn real_target_helper_allocates_nothing() {
    let mut target = 7u8;
    let mut cx = Context::from_waker(Waker::noop());
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|flag| flag.set(true));
    let returned = {
        let mut future = pin!(target.named_byte_unpin());
        future.as_mut().poll(&mut cx)
    };
    COUNTING.with(|flag| flag.set(false));
    assert_eq!(returned, Poll::Ready(8));
    assert_eq!(target, 8);
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
}

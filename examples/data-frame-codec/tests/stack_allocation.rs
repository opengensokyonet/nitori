#![feature(impl_trait_in_assoc_type, yield_expr)]
#![deny(unsafe_op_in_unsafe_fn)]
use nitori_call::{Call, CallPoll, Direct, Lend, Receiver, ReceiverAccess, ReceiverFamily, call};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    future::{Future, poll_fn},
    ops::ControlFlow,
    pin::pin,
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
/// A family whose view holds a byte that each read increments.
trait Counter: ReceiverFamily {
    fn bump<'v>(view: std::pin::Pin<&mut Self::ReceiverView<'v>>) -> u8
    where
        Self: 'v;
}
impl Counter for Direct<u8> {
    fn bump<'v>(view: std::pin::Pin<&mut Self::ReceiverView<'v>>) -> u8
    where
        Self: 'v,
    {
        let mut target = view.get_pin_mut();
        *target += 1;
        *target
    }
}
#[call]
async fn named_byte<A: ReceiverAccess<Family: Counter>>(io: &mut A) -> u8 {
    poll_fn(|cx| {
        io.poll_access(cx, |access, _| {
            Poll::Ready(A::Family::bump(access.into_pin()))
        })
    })
    .await
}
#[call(yields = u8)]
async fn sum_bytes<A: ReceiverAccess<Family: Counter>>(io: &mut A) -> usize {
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
}
#[test]
fn construction_ready_pending_emit_completion_and_drop_allocate_nothing() {
    let mut target = 0u8;
    let mut cx = Context::from_waker(Waker::noop());
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|flag| flag.set(true));
    let (emitted, waits, returned) = {
        let mut call = pin!(Call::<Direct<u8>, _>::new(SumBytes::new()));
        let mut emitted = 0;
        let mut waits = 0;
        let returned = loop {
            let view = pin!(std::pin::Pin::new(&mut target).view());
            match call.as_mut().poll_with(Some(Lend::Fixed(view)), &mut cx) {
                CallPoll::Pending => waits += 1,
                CallPoll::Ready(ControlFlow::Continue(_)) => emitted += 1,
                CallPoll::Ready(ControlFlow::Break(value)) => break value,
                CallPoll::Releasable => unreachable!("a fixed view is not offered back"),
                CallPoll::NeedsView => unreachable!("a view is always lent"),
            }
        };
        (emitted, waits, returned)
    };
    COUNTING.with(|flag| flag.set(false));
    assert_eq!((emitted, waits, returned, target), (4, 4, 10, 4));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
}

#[test]
fn a_host_method_allocates_nothing() {
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

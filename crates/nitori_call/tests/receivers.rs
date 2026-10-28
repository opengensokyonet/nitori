#![feature(impl_trait_in_assoc_type)]
mod support;

use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use nitori_call::{
    Call, CallPoll, Child, Direct, Emitter, Lend, Operation, Receiver, ReceiverAccess, Receivers,
    Start, ViewLoan, direct_receiver,
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    convert::Infallible,
    future::{Future, poll_fn},
    pin::{Pin, pin},
    rc::Rc,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
};
use support::*;

type Memory<'a> = Direct<&'a [u8]>;

fn assert_send<T: Send>(_: &T) {}

/// Reads one byte from each receiver, the first one first.
struct AddBytes;
impl<A: Receivers, B: Receivers> Operation<(A, B)> for AddBytes {
    type Output = u16;
    type Yield = Infallible;
}
impl<'s, A, B> Start<'s, (A, B)> for AddBytes
where
    A: ReceiverAccess<Family: Read<Error = Infallible>>,
    B: ReceiverAccess<Family: Read<Error = Infallible>>,
{
    type Fut = impl Future<Output = u16>;
    fn start(self, (mut a, mut b): (A, B), _: Emitter<'s, Infallible>) -> Self::Fut {
        async move {
            let [x] = read_array::<1, _>(&mut a).await.unwrap();
            let [y] = read_array::<1, _>(&mut b).await.unwrap();
            u16::from(x) + u16::from(y)
        }
    }
}

#[test]
fn each_slot_asks_for_its_own_view() {
    let mut first = b"\x01".as_slice();
    let mut second = b"\x02".as_slice();
    let mut call = pin!(Call::<(Memory, Memory), _>::new(AddBytes));
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(
        call.as_mut().poll_return_with_views((None, None), &mut cx),
        CallPoll::NeedsView
    );
    assert_eq!(call.views_needed(), (true, false));
    {
        let mut view = pin!(Pin::new(&mut first).view());
        let lends = (Some(Lend::Fixed(ViewLoan::new(view.as_mut()))), None);
        assert_eq!(
            call.as_mut().poll_return_with_views(lends, &mut cx),
            CallPoll::NeedsView
        );
    }
    assert_eq!(call.views_needed(), (false, true));
    let mut view = pin!(Pin::new(&mut second).view());
    let lends = (None, Some(Lend::Fixed(ViewLoan::new(view.as_mut()))));
    assert_eq!(
        call.as_mut().poll_return_with_views(lends, &mut cx),
        CallPoll::Ready(3)
    );
    assert_eq!(call.views_needed(), (false, false));
}

/// Bytes shared within one thread: neither the resource nor its views are Send.
#[derive(Default)]
struct Local(Rc<RefCell<VecDeque<u8>>>);
direct_receiver!(impl [] for Local);
impl Read for Direct<Local> {
    type Error = Infallible;
    fn poll_read<'visit, Output: bytes::BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'visit,
    {
        let mut data = host.get_pin_mut().get_mut().0.borrow_mut();
        if data.is_empty() && destination.has_remaining_mut() {
            return Poll::Pending;
        }
        let count = data.len().min(destination.remaining_mut());
        for byte in data.drain(..count) {
            destination.put_u8(byte);
        }
        Poll::Ready(Ok(count))
    }
}
impl Local {
    fn with(bytes: &[u8]) -> Self {
        let local = Self::default();
        local.0.borrow_mut().extend(bytes);
        local
    }
}

#[test]
fn a_call_over_a_resource_that_is_not_send_moves_between_threads() {
    let mut call = Box::pin(Call::<(Direct<Local>, Memory), _>::new(AddBytes));
    assert_send(&call);
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut local = Local::with(b"\x05");
        let mut view = pin!(Pin::new(&mut local).view());
        let lends = (Some(Lend::Fixed(ViewLoan::new(view.as_mut()))), None);
        assert_eq!(
            call.as_mut().poll_return_with_views(lends, &mut cx),
            CallPoll::NeedsView
        );
    }
    let result = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut memory = b"\x07".as_slice();
                let mut view = pin!(Pin::new(&mut memory).view());
                let lends = (None, Some(Lend::Fixed(ViewLoan::new(view.as_mut()))));
                call.as_mut()
                    .poll_return_with_views(lends, &mut Context::from_waker(Waker::noop()))
            })
            .join()
            .unwrap()
    });
    assert_eq!(result, CallPoll::Ready(12));
}

/// Asks for both views at once from two scoped threads, then reads both on
/// the polling thread.
struct AskTogether;
impl<A: Receivers, B: Receivers> Operation<(A, B)> for AskTogether {
    type Output = u16;
    type Yield = Infallible;
}
impl<'s, A, B> Start<'s, (A, B)> for AskTogether
where
    A: ReceiverAccess<Family: Read<Error = Infallible>> + Send,
    B: ReceiverAccess<Family: Read<Error = Infallible>> + Send,
{
    type Fut = impl Future<Output = u16>;
    fn start(self, (mut a, mut b): (A, B), _: Emitter<'s, Infallible>) -> Self::Fut {
        async move {
            let mut asked = false;
            poll_fn(|cx| {
                if std::mem::replace(&mut asked, true) {
                    return Poll::Ready(());
                }
                let waker = cx.waker().clone();
                let barrier = Barrier::new(2);
                let ask = |io: &mut dyn FnMut(&mut Context<'_>) -> Poll<()>| {
                    barrier.wait();
                    io(&mut Context::from_waker(&waker))
                };
                std::thread::scope(|scope| {
                    let first =
                        scope.spawn(|| ask(&mut |cx| a.poll_access(cx, |_, _| Poll::Ready(()))));
                    let second =
                        scope.spawn(|| ask(&mut |cx| b.poll_access(cx, |_, _| Poll::Ready(()))));
                    assert!(first.join().unwrap().is_pending());
                    assert!(second.join().unwrap().is_pending());
                });
                Poll::Pending
            })
            .await;
            let [x] = read_array::<1, _>(&mut a).await.unwrap();
            let [y] = read_array::<1, _>(&mut b).await.unwrap();
            u16::from(x) + u16::from(y)
        }
    }
}

#[test]
fn slots_on_different_threads_ask_at_the_same_time() {
    let mut first = b"\x03".as_slice();
    let mut second = b"\x04".as_slice();
    let mut call = pin!(Call::<(Memory, Memory), _>::new(AskTogether));
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(
        call.as_mut().poll_return_with_views((None, None), &mut cx),
        CallPoll::NeedsView
    );
    assert_eq!(call.views_needed(), (true, true));
    let mut a = pin!(Pin::new(&mut first).view());
    let mut b = pin!(Pin::new(&mut second).view());
    let lends = (
        Some(Lend::Fixed(ViewLoan::new(a.as_mut()))),
        Some(Lend::Fixed(ViewLoan::new(b.as_mut()))),
    );
    assert_eq!(
        call.as_mut().poll_return_with_views(lends, &mut cx),
        CallPoll::Ready(7)
    );
}

/// Reads one byte from each of three receivers in branches of a
/// `FuturesUnordered`, counting how often each branch is polled.
struct ReadInBranches(Arc<[AtomicUsize; 3]>);
impl<A: Receivers, B: Receivers, C: Receivers> Operation<(A, B, C)> for ReadInBranches {
    type Output = u16;
    type Yield = Infallible;
}
impl<'s, A> Start<'s, (A, A, A)> for ReadInBranches
where
    A: ReceiverAccess<Family: Read<Error = Infallible>> + 's,
{
    type Fut = impl Future<Output = u16>;
    fn start(self, (a, b, c): (A, A, A), _: Emitter<'s, Infallible>) -> Self::Fut {
        async move {
            let mut branches = FuturesUnordered::new();
            for (index, mut io) in [a, b, c].into_iter().enumerate() {
                let polls = self.0.clone();
                branches.push(
                    async move {
                        let mut read = pin!(read_array::<1, _>(&mut io));
                        let [byte] = poll_fn(|cx| {
                            polls[index].fetch_add(1, Ordering::Relaxed);
                            read.as_mut().poll(cx)
                        })
                        .await
                        .unwrap();
                        u16::from(byte)
                    }
                    .boxed_local(),
                );
            }
            let mut sum = 0;
            while let Some(byte) = branches.next().await {
                sum += byte;
            }
            sum
        }
    }
}

#[test]
fn a_view_wakes_only_the_branches_waiting_for_its_slot() {
    let polls = Arc::new([const { AtomicUsize::new(0) }; 3]);
    let counts = || polls.each_ref().map(|count| count.load(Ordering::Relaxed));
    let mut call = pin!(Call::<(Memory, Memory, Memory), _>::new(ReadInBranches(
        polls.clone()
    )));
    let (wakes, waker) = WakeCount::new();
    let mut cx = Context::from_waker(&waker);
    assert_eq!(
        call.as_mut()
            .poll_return_with_views((None, None, None), &mut cx),
        CallPoll::NeedsView
    );
    assert_eq!(call.views_needed(), (true, true, true));
    assert_eq!(counts(), [1, 1, 1]);
    // Having polled every branch, `FuturesUnordered` yields by waking the
    // caller; no wake after this one comes from the call.
    let yielded = wakes.get();
    // No view: no branch is polled again.
    assert_eq!(
        call.as_mut()
            .poll_return_with_views((None, None, None), &mut cx),
        CallPoll::NeedsView
    );
    assert_eq!(counts(), [1, 1, 1]);
    let mut first = b"\x01".as_slice();
    {
        let mut view = pin!(Pin::new(&mut first).view());
        let lends = (Some(Lend::Fixed(ViewLoan::new(view.as_mut()))), None, None);
        assert_eq!(
            call.as_mut().poll_return_with_views(lends, &mut cx),
            CallPoll::NeedsView
        );
    }
    assert_eq!(counts(), [2, 1, 1]);
    assert_eq!(call.views_needed(), (false, true, true));
    let (mut second, mut third) = (b"\x02".as_slice(), b"\x04".as_slice());
    let mut b = pin!(Pin::new(&mut second).view());
    let mut c = pin!(Pin::new(&mut third).view());
    let lends = (
        None,
        Some(Lend::Fixed(ViewLoan::new(b.as_mut()))),
        Some(Lend::Fixed(ViewLoan::new(c.as_mut()))),
    );
    assert_eq!(
        call.as_mut().poll_return_with_views(lends, &mut cx),
        CallPoll::Ready(7)
    );
    assert_eq!(counts(), [2, 2, 2]);
    // Replaying the branches never woke the caller.
    assert_eq!(wakes.get(), yielded);
}

/// Reads a byte from the first receiver, offers its view back, then reads
/// another; the second receiver is only read at the end.
struct ReleaseFirst;
impl<A: Receivers, B: Receivers> Operation<(A, B)> for ReleaseFirst {
    type Output = u16;
    type Yield = Infallible;
}
impl<'s, A, B> Start<'s, (A, B)> for ReleaseFirst
where
    A: ReceiverAccess<Family: Read<Error = Infallible>>,
    B: ReceiverAccess<Family: Read<Error = Infallible>>,
{
    type Fut = impl Future<Output = u16>;
    fn start(self, (mut a, mut b): (A, B), _: Emitter<'s, Infallible>) -> Self::Fut {
        async move {
            let [x] = read_array::<1, _>(&mut a).await.unwrap();
            a.release_view().await;
            let [y] = read_array::<1, _>(&mut a).await.unwrap();
            let [z] = read_array::<1, _>(&mut b).await.unwrap();
            u16::from(x) + u16::from(y) + u16::from(z)
        }
    }
}

#[test]
fn each_slot_offers_its_own_view_back() {
    let mut first = b"\x01\x02".as_slice();
    let mut second = b"\x04".as_slice();
    let mut call = pin!(Call::<(Memory, Memory), _>::new(ReleaseFirst));
    let mut cx = Context::from_waker(Waker::noop());
    let mut b = pin!(Pin::new(&mut second).view());
    {
        let mut a = pin!(Pin::new(&mut first).view());
        let lends = (
            Some(Lend::Releasable(ViewLoan::new(a.as_mut()))),
            Some(Lend::Fixed(ViewLoan::new(b.as_mut()))),
        );
        assert_eq!(
            call.as_mut().poll_return_with_views(lends, &mut cx),
            CallPoll::Releasable
        );
    }
    assert_eq!(call.views_offered(), (true, false));
    // Release the first view: the operation asks for it again.
    let lends = (None, Some(Lend::Fixed(ViewLoan::new(b.as_mut()))));
    assert_eq!(
        call.as_mut().poll_return_with_views(lends, &mut cx),
        CallPoll::NeedsView
    );
    assert_eq!(call.views_offered(), (false, false));
    assert_eq!(call.views_needed(), (true, false));
    let mut a = pin!(Pin::new(&mut first).view());
    let lends = (
        Some(Lend::Fixed(ViewLoan::new(a.as_mut()))),
        Some(Lend::Fixed(ViewLoan::new(b.as_mut()))),
    );
    assert_eq!(
        call.as_mut().poll_return_with_views(lends, &mut cx),
        CallPoll::Ready(7)
    );
}

/// Runs `AddBytes` as a child over borrows of both slots, twice.
struct AddTwice;
impl<A: Receivers, B: Receivers> Operation<(A, B)> for AddTwice {
    type Output = u16;
    type Yield = Infallible;
}
impl<'s, A, B> Start<'s, (A, B)> for AddTwice
where
    A: ReceiverAccess<Family: Read<Error = Infallible>>,
    B: ReceiverAccess<Family: Read<Error = Infallible>>,
{
    type Fut = impl Future<Output = u16>;
    fn start(self, (mut a, mut b): (A, B), _: Emitter<'s, Infallible>) -> Self::Fut {
        async move {
            let first = Child::new(AddBytes, (&mut a, &mut b)).await;
            first + Child::new(AddBytes, (&mut a, &mut b)).await
        }
    }
}

#[test]
fn a_child_runs_over_several_accesses() {
    fn run<F: Read<Error = Infallible>>(mut first: impl Receiver<Family = F> + Unpin) -> u16 {
        let mut second = b"\x10\x20".as_slice();
        let mut call = pin!(Call::<(F, Memory), _>::new(AddTwice));
        assert_send(&*call);
        let mut a = pin!(Pin::new(&mut first).view());
        let mut b = pin!(Pin::new(&mut second).view());
        let lends = (
            Some(Lend::Fixed(ViewLoan::new(a.as_mut()))),
            Some(Lend::Fixed(ViewLoan::new(b.as_mut()))),
        );
        match call
            .as_mut()
            .poll_return_with_views(lends, &mut Context::from_waker(Waker::noop()))
        {
            CallPoll::Ready(sum) => sum,
            poll => panic!("unexpected {poll:?}"),
        }
    }
    assert_eq!(run(b"\x01\x02".as_slice()), 0x33);
}

#[test]
fn a_call_over_receivers_that_are_not_send_is_send() {
    fn check<R: Receivers>()
    where
        AddBytes: Operation<R> + for<'s> Start<'s, R::Slots<'s>>,
    {
    }
    check::<(Direct<Local>, Direct<Local>)>();
    let call = Call::<(Direct<Local>, Direct<Local>), _>::new(AddBytes);
    assert_send(&call);
}

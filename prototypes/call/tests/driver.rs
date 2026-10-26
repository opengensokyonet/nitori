//! A driver that provides no view of its own: it acquires a lock only for the
//! polls whose operation reads, so `reader.read_record(..).await` works.
#![feature(impl_trait_in_assoc_type, yield_expr)]
mod support;

use bytes::BufMut;
use futures_core::Stream;
use nitori_call_prototype::{
    AcquisitionState, CallExecutor, Operation, ReceiverAccess, ReceiverFamily, ReceiverSlot, Start,
    call, drive,
};
use std::{
    cell::Cell,
    collections::VecDeque,
    convert::Infallible,
    future::{Future, poll_fn},
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use support::{Read, ReadError, poll_once, read_array, read_exact};
use tokio::sync::{Mutex, MutexGuard};

/// Bytes shared behind an async lock, as a connection is shared by streams.
#[derive(Default)]
struct Pipe {
    data: VecDeque<u8>,
    reader: Option<Waker>,
}
impl Pipe {
    fn push(&mut self, bytes: &[u8]) {
        self.data.extend(bytes);
        if let Some(waker) = self.reader.take() {
            waker.wake();
        }
    }
}

/// Views of this family are lock guards.
struct Locked;
struct PipeGuard<'v>(MutexGuard<'v, Pipe>);
impl ReceiverFamily for Locked {
    type ReceiverView<'v> = PipeGuard<'v>;
}
impl Read for Locked {
    type Error = Infallible;
    fn poll_read<'visit, Output: BufMut + ?Sized>(
        host: Pin<&mut PipeGuard<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'visit,
    {
        let pipe = &mut *host.get_mut().0;
        if pipe.data.is_empty() && destination.has_remaining_mut() {
            pipe.reader = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let count = pipe.data.len().min(destination.remaining_mut());
        for byte in pipe.data.drain(..count) {
            destination.put_u8(byte);
        }
        Poll::Ready(Ok(count))
    }
}

/// A reading handle on the shared pipe. It holds no view: each poll that
/// reads acquires the lock and releases it before returning.
struct Reader<'p> {
    pipe: &'p Mutex<Pipe>,
    acquisitions: &'p Cell<usize>,
}
type Event<Op> = ControlFlow<<Op as Operation<Locked>>::Output, <Op as Operation<Locked>>::Yield>;
impl<'p> CallExecutor for Reader<'p> {
    type Family = Locked;
    type Execution<'a, Op>
        = impl Future<Output = <Op as Operation<Locked>>::Output> + Stream<Item = Event<Op>>
    where
        Self: 'a,
        Op: Operation<Locked> + for<'s> Start<'s, ReceiverSlot<'s, Locked>> + 'a;
    fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> Self::Execution<'a, Op>
    where
        Op: Operation<Locked> + for<'s> Start<'s, ReceiverSlot<'s, Locked>> + 'a,
    {
        let Reader { pipe, acquisitions } = *self.get_mut();
        let acquisition = AcquisitionState::<Locked, _, _>::new(move || {
            acquisitions.set(acquisitions.get() + 1);
            async move { PipeGuard(pipe.lock().await) }
        });
        drive(acquisition, operation)
    }
}

type Record = Result<usize, ReadError<Infallible>>;

#[call]
async fn read_record(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    output: &mut Vec<u8>,
) -> Record {
    let [length] = read_array::<1, _>(io).await?;
    read_exact(io, output, length.into()).await?;
    Ok(length.into())
}

#[test]
fn a_driver_runs_operations_and_holds_the_lock_only_within_a_poll() {
    let pipe = Mutex::new(Pipe::default());
    let acquisitions = Cell::new(0);
    let mut reader = Reader {
        pipe: &pipe,
        acquisitions: &acquisitions,
    };
    let mut output = Vec::new();
    {
        let mut record = pin!(reader.read_record_unpin(&mut output));
        // The read waits for data without holding the lock.
        assert!(poll_once(record.as_mut()).is_pending());
        assert_eq!(acquisitions.get(), 1);
        pipe.try_lock()
            .expect("the lock is released")
            .push(b"\x02hi");
        assert_eq!(poll_once(record.as_mut()), Poll::Ready(Ok(2)));
        assert_eq!(acquisitions.get(), 2);
    }
    assert_eq!(output, b"hi");
}

#[test]
fn a_waiting_driver_is_not_woken_by_its_own_request() {
    let pipe = Mutex::new(Pipe::default());
    let acquisitions = Cell::new(0);
    let mut reader = Reader {
        pipe: &pipe,
        acquisitions: &acquisitions,
    };
    let (wakes, waker) = support::WakeCount::new();
    let mut cx = Context::from_waker(&waker);
    let mut output = Vec::new();
    let mut record = pin!(reader.read_record_unpin(&mut output));
    // A runtime would repoll a task woken during its poll, forever.
    assert!(record.as_mut().poll(&mut cx).is_pending());
    assert!(record.as_mut().poll(&mut cx).is_pending());
    assert_eq!(wakes.get(), 0);
    pipe.try_lock().unwrap().push(b"\x01k");
    assert_eq!(wakes.get(), 1);
    assert_eq!(record.as_mut().poll(&mut cx), Poll::Ready(Ok(1)));
}

#[test]
fn a_contended_acquisition_is_kept_until_the_lock_is_free() {
    let pipe = Mutex::new(Pipe::default());
    let acquisitions = Cell::new(0);
    let mut reader = Reader {
        pipe: &pipe,
        acquisitions: &acquisitions,
    };
    let mut output = Vec::new();
    {
        let mut holder = pipe.try_lock().unwrap();
        holder.push(b"\x01z");
        let mut record = pin!(reader.read_record_unpin(&mut output));
        assert!(poll_once(record.as_mut()).is_pending());
        drop(holder);
        // The same acquisition completes; no second one starts.
        assert_eq!(poll_once(record.as_mut()), Poll::Ready(Ok(1)));
        assert_eq!(acquisitions.get(), 1);
    }
    assert_eq!(output, b"z");
}

/// A permission that opens later; waiting for it needs no resource.
#[derive(Default)]
struct Gate {
    open: Cell<bool>,
}
impl Gate {
    async fn wait(&self) {
        poll_fn(|_| match self.open.get() {
            true => Poll::Ready(()),
            false => Poll::Pending,
        })
        .await
    }
}

#[call]
async fn read_record_after(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    gate: &Gate,
    output: &mut Vec<u8>,
) -> Record {
    gate.wait().await;
    read_record(io, output).await
}

#[test]
fn waiting_for_something_else_acquires_nothing() {
    let pipe = Mutex::new(Pipe::default());
    pipe.try_lock().unwrap().push(b"\x01q");
    let acquisitions = Cell::new(0);
    let mut reader = Reader {
        pipe: &pipe,
        acquisitions: &acquisitions,
    };
    let gate = Gate::default();
    let mut output = Vec::new();
    {
        let mut record = pin!(reader.read_record_after_unpin(&gate, &mut output));
        assert!(poll_once(record.as_mut()).is_pending());
        assert!(poll_once(record.as_mut()).is_pending());
        assert_eq!(acquisitions.get(), 0);
        gate.open.set(true);
        assert_eq!(poll_once(record.as_mut()), Poll::Ready(Ok(1)));
        assert_eq!(acquisitions.get(), 1);
    }
    assert_eq!(output, b"q");
}

/// Records as items until an empty one, then how many there were.
#[call(yields = Vec<u8>)]
async fn read_records(io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>) -> usize {
    let mut count = 0;
    loop {
        let mut record = Vec::new();
        if read_record(io, &mut record).await == Ok(0) {
            return count;
        }
        count += 1;
        yield record;
    }
}

#[test]
fn an_executor_delivers_items_as_a_stream_and_releases_the_lock_between_them() {
    let pipe = Mutex::new(Pipe::default());
    pipe.try_lock().unwrap().push(b"\x01a\x01b");
    let acquisitions = Cell::new(0);
    let mut reader = Reader {
        pipe: &pipe,
        acquisitions: &acquisitions,
    };
    let mut records = pin!(reader.read_records_unpin());
    let mut cx = Context::from_waker(Waker::noop());
    let mut next = || records.as_mut().poll_next(&mut cx);
    assert_eq!(
        next(),
        Poll::Ready(Some(ControlFlow::Continue(b"a".to_vec())))
    );
    // Between items the lock is free.
    assert!(pipe.try_lock().is_ok());
    assert_eq!(
        next(),
        Poll::Ready(Some(ControlFlow::Continue(b"b".to_vec())))
    );
    assert!(next().is_pending());
    pipe.try_lock().unwrap().push(b"\x00");
    assert_eq!(next(), Poll::Ready(Some(ControlFlow::Break(2))));
    assert_eq!(next(), Poll::Ready(None));
    assert_eq!(acquisitions.get(), 4);
}

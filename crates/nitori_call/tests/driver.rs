//! A driver that provides no view of its own: it acquires a lock only for the
//! polls whose operation reads, so `reader.read_record(..).await` works.
#![feature(impl_trait_in_assoc_type, yield_expr)]
mod support;

use bytes::BufMut;
use futures_core::{FusedFuture, FusedStream, Stream};
use nitori_call::{
    AcquisitionState, CallExecutor, Lend, Operation, ReceiverAccess, ReceiverFamily, RootOperation,
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
use support::{Read, ReadError, WakeCount, only_when_woken, poll_once, read_array, read_exact};
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
        Op: RootOperation<Locked> + 'a;
    fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> Self::Execution<'a, Op>
    where
        Op: RootOperation<Locked> + 'a,
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

/// Writes through a borrow of its own local before and after a wait that
/// needs no resource.
#[call]
async fn read_record_counted(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    gate: &Gate,
) -> Result<Vec<u8>, ReadError<Infallible>> {
    let mut record = vec![0];
    let output = &mut record;
    output.clear();
    gate.wait().await;
    read_record(io, output).await?;
    Ok(record)
}

#[test]
fn shared_references_to_running_executions_leave_them_intact() {
    let gate = Gate::default();
    let mut input = b"\x01q".as_slice();
    let pipe = Mutex::new(Pipe::default());
    pipe.try_lock().unwrap().push(b"\x01r");
    let acquisition =
        AcquisitionState::<Locked, _, _>::new(|| async { PipeGuard(pipe.lock().await) });
    {
        let mut bound = pin!(input.read_record_counted_unpin(&gate));
        let mut driven = pin!(drive(acquisition, ReadRecordCounted::new(&gate)));
        assert!(poll_once(bound.as_mut()).is_pending());
        assert!(poll_once(driven.as_mut()).is_pending());
        // Safe code can borrow pinned executions.
        assert!(!FusedFuture::is_terminated(&*bound));
        assert!(!FusedFuture::is_terminated(&*driven));
        assert_eq!(format!("{bound:?}"), "BoundCall { terminated: false, .. }");
        assert_eq!(format!("{driven:?}"), "Drive { terminated: false, .. }");
        gate.open.set(true);
        assert_eq!(poll_once(bound.as_mut()), Poll::Ready(Ok(b"q".to_vec())));
        assert_eq!(poll_once(driven.as_mut()), Poll::Ready(Ok(b"r".to_vec())));
        assert!(FusedFuture::is_terminated(&*bound));
        assert!(FusedStream::is_terminated(&*driven));
        assert_eq!(driven.size_hint(), (0, Some(0)));
    }
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

/// Reads the records as a stream inside the operation, so the records ready
/// in one poll share one acquisition.
#[call]
async fn collect_records(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
) -> Vec<Vec<u8>> {
    let mut records = pin!(io.read_records());
    let mut collected = Vec::new();
    while let Some(ControlFlow::Continue(record)) = records.as_mut().next().await {
        collected.push(record);
    }
    collected
}

#[test]
fn items_read_inside_an_operation_share_an_acquisition() {
    let pipe = Mutex::new(Pipe::default());
    pipe.try_lock().unwrap().push(b"\x01a\x01b\x00");
    let acquisitions = Cell::new(0);
    let mut reader = Reader {
        pipe: &pipe,
        acquisitions: &acquisitions,
    };
    let mut records = pin!(reader.collect_records_unpin());
    assert_eq!(
        poll_once(records.as_mut()),
        Poll::Ready(vec![b"a".to_vec(), b"b".to_vec()])
    );
    assert_eq!(acquisitions.get(), 1);
}

/// Appends bytes to a log.
trait Log: ReceiverFamily {
    fn append<'visit>(host: Pin<&mut Self::ReceiverView<'visit>>, bytes: &[u8])
    where
        Self: 'visit;
}
fn append_log<'a, A: ReceiverAccess<Family: Log>>(
    io: &'a mut A,
    bytes: &'a [u8],
) -> impl Future<Output = ()> + 'a {
    poll_fn(move |cx| {
        io.poll_access(cx, |access, _| {
            <A::Family as Log>::append(access.into_pin(), bytes);
            Poll::Ready(())
        })
    })
}

/// Reads from the pipe, waiting while it is empty.
fn poll_pipe(
    pipe: &mut Pipe,
    cx: &mut Context<'_>,
    destination: &mut (impl BufMut + ?Sized),
) -> Poll<Result<usize, Infallible>> {
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

/// Copies a record from the pipe to the log, with one access for both.
#[call]
async fn copy_record(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible> + Log>,
) -> Record {
    let mut record = Vec::new();
    let length = read_record(io, &mut record).await?;
    append_log(io, &record).await;
    Ok(length)
}

/// One family over both resources: each view holds both guards, which are
/// acquired together.
struct PipeAndLog;
struct BothGuards<'v>(MutexGuard<'v, Pipe>, MutexGuard<'v, Vec<u8>>);
impl ReceiverFamily for PipeAndLog {
    type ReceiverView<'v> = BothGuards<'v>;
}
impl Read for PipeAndLog {
    type Error = Infallible;
    fn poll_read<'visit, Output: BufMut + ?Sized>(
        host: Pin<&mut BothGuards<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'visit,
    {
        poll_pipe(&mut host.get_mut().0, cx, destination)
    }
}
impl Log for PipeAndLog {
    fn append<'visit>(host: Pin<&mut BothGuards<'visit>>, bytes: &[u8])
    where
        Self: 'visit,
    {
        host.get_mut().1.extend_from_slice(bytes);
    }
}

#[test]
fn two_resources_acquired_together_as_one_family() {
    let pipe = Mutex::new(Pipe::default());
    let log = Mutex::new(Vec::new());
    let acquisitions = Cell::new(0);
    let acquisition = AcquisitionState::<PipeAndLog, _, _>::new(|| {
        acquisitions.set(acquisitions.get() + 1);
        // Always in the same order, so that two such families cannot deadlock.
        async { BothGuards(pipe.lock().await, log.lock().await) }
    });
    let mut copy = pin!(drive(acquisition, CopyRecord::new()));
    assert!(poll_once(copy.as_mut()).is_pending());
    // Waiting for data holds neither lock.
    assert!(log.try_lock().is_ok());
    pipe.try_lock().unwrap().push(b"\x02hi");
    assert_eq!(poll_once(copy.as_mut()), Poll::Ready(Ok(2)));
    assert_eq!(acquisitions.get(), 2);
    assert_eq!(*log.try_lock().unwrap(), b"hi");
}

/// Views of the log alone.
struct Logged;
struct LogGuard<'v>(MutexGuard<'v, Vec<u8>>);
impl ReceiverFamily for Logged {
    type ReceiverView<'v> = LogGuard<'v>;
}
impl Log for Logged {
    fn append<'visit>(host: Pin<&mut LogGuard<'visit>>, bytes: &[u8])
    where
        Self: 'visit,
    {
        host.get_mut().0.extend_from_slice(bytes);
    }
}

#[call]
async fn write_log(io: &mut impl ReceiverAccess<Family: Log>, bytes: &[u8]) {
    append_log(io, bytes).await;
}

/// Copies a record from the pipe to a log that it acquires separately.
#[call]
async fn copy_record_to(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    log: &Mutex<Vec<u8>>,
) -> Record {
    let mut record = Vec::new();
    let length = read_record(io, &mut record).await?;
    let acquisition =
        AcquisitionState::<Logged, _, _>::new(|| async { LogGuard(log.lock().await) });
    drive(acquisition, WriteLog::new(&record)).await;
    Ok(length)
}

#[test]
fn a_second_resource_is_driven_inside_the_operation() {
    let pipe = Mutex::new(Pipe::default());
    let log = Mutex::new(Vec::new());
    let acquisitions = Cell::new(0);
    let mut reader = Reader {
        pipe: &pipe,
        acquisitions: &acquisitions,
    };
    let mut copy = pin!(reader.copy_record_to_unpin(&log));
    pipe.try_lock().unwrap().push(b"\x02ok");
    {
        // The log is busy: the record has been read, and the pipe is free
        // while the operation waits for the log.
        let busy = log.try_lock().unwrap();
        assert!(poll_once(copy.as_mut()).is_pending());
        assert!(pipe.try_lock().is_ok());
        drop(busy);
    }
    assert_eq!(poll_once(copy.as_mut()), Poll::Ready(Ok(2)));
    assert_eq!(*log.try_lock().unwrap(), b"ok");
}

/// Reads one byte, using the resource only on polls while `wanted` is set.
#[call]
async fn read_when_wanted<A: ReceiverAccess<Family: Read<Error = Infallible>>>(
    io: &mut A,
    wanted: &Cell<bool>,
) -> u8 {
    let mut byte = [0];
    poll_fn(|cx| match wanted.get() {
        false => Poll::Pending,
        true => io.poll_access(cx, |access, cx| {
            <A::Family as Read>::poll_read(access.into_pin(), cx, &mut byte.as_mut_slice())
        }),
    })
    .await
    .unwrap();
    byte[0]
}

#[test]
fn an_acquisition_granted_while_unneeded_is_released() {
    let pipe = Mutex::new(Pipe::default());
    let wanted = Cell::new(true);
    let acquisitions = Cell::new(0);
    let acquisition = AcquisitionState::<Locked, _, _>::new(|| {
        acquisitions.set(acquisitions.get() + 1);
        async { PipeGuard(pipe.lock().await) }
    });
    let mut read = pin!(drive(acquisition, ReadWhenWanted::new(&wanted)));
    let holder = pipe.try_lock().unwrap();
    // The operation asks while the lock is held: the acquisition queues.
    assert!(poll_once(read.as_mut()).is_pending());
    // It stops needing the lock. While the lock stays held, the acquisition
    // keeps its place.
    wanted.set(false);
    assert!(poll_once(read.as_mut()).is_pending());
    assert_eq!(acquisitions.get(), 1);
    // The lock is granted to it while the operation waits for something
    // else, and released by the next poll.
    drop(holder);
    assert!(poll_once(read.as_mut()).is_pending());
    assert!(pipe.try_lock().is_ok());
    // Asking again acquires anew.
    wanted.set(true);
    pipe.try_lock().unwrap().push(b"z");
    assert_eq!(poll_once(read.as_mut()), Poll::Ready(b'z'));
    assert_eq!(acquisitions.get(), 2);
}

#[test]
fn a_pending_acquisition_advanced_while_unneeded_completes_later() {
    let pipe = Mutex::new(Pipe::default());
    pipe.try_lock().unwrap().push(b"y");
    let wanted = Cell::new(true);
    let acquisitions = Cell::new(0);
    let acquisition = AcquisitionState::<Locked, _, _>::new(|| {
        acquisitions.set(acquisitions.get() + 1);
        async { PipeGuard(pipe.lock().await) }
    });
    let mut read = pin!(drive(acquisition, ReadWhenWanted::new(&wanted)));
    let holder = pipe.try_lock().unwrap();
    assert!(poll_once(read.as_mut()).is_pending());
    wanted.set(false);
    assert!(poll_once(read.as_mut()).is_pending());
    // Wanted again before the lock is free: the same acquisition completes.
    wanted.set(true);
    drop(holder);
    assert_eq!(poll_once(read.as_mut()), Poll::Ready(b'y'));
    assert_eq!(acquisitions.get(), 1);
}

/// Reads a byte, computes without needing the pipe, then reads another.
#[call]
async fn read_compute_read(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    compute: &dyn Fn(),
) -> Result<[u8; 2], ReadError<Infallible>> {
    let [first] = read_array::<1, _>(io).await?;
    io.release_view().await;
    compute();
    let [second] = read_array::<1, _>(io).await?;
    Ok([first, second])
}

/// The same, with the release point inside a branch of an executor that
/// polls it with a waker of its own.
#[call]
async fn read_compute_read_in_branch(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    compute: &dyn Fn(),
) -> Result<[u8; 2], ReadError<Infallible>> {
    only_when_woken(read_compute_read(io, compute)).await
}

/// Runs `operation` in one poll with `acquisition` and reports whether the
/// lock was free during the computation.
fn lock_free_while_computing<'p, S>(pipe: &'p Mutex<Pipe>, acquisition: S, in_branch: bool) -> bool
where
    S: nitori_call::ViewAcquisition<'p, Family = Locked>,
{
    pipe.try_lock().unwrap().push(b"ab");
    let free = Cell::new(None);
    let compute = || free.set(Some(pipe.try_lock().is_ok()));
    let (wakes, waker) = WakeCount::new();
    let mut cx = Context::from_waker(&waker);
    let result = match in_branch {
        false => pin!(drive(acquisition, ReadComputeRead::new(&compute))).poll(&mut cx),
        true => pin!(drive(acquisition, ReadComputeReadInBranch::new(&compute))).poll(&mut cx),
    };
    // The computation runs in the same poll: the task is not suspended.
    assert_eq!(result, Poll::Ready(Ok(*b"ab")));
    assert_eq!(wakes.get(), 0);
    free.get().unwrap()
}

#[test]
fn a_fixed_view_is_held_through_a_release_point() {
    let pipe = Mutex::new(Pipe::default());
    let acquisitions = Cell::new(0);
    let acquisition = AcquisitionState::<Locked, _, _>::new(|| {
        acquisitions.set(acquisitions.get() + 1);
        async { PipeGuard(pipe.lock().await) }
    });
    assert!(!lock_free_while_computing(&pipe, acquisition, false));
    assert_eq!(acquisitions.get(), 1);
}

#[test]
fn a_releasable_view_is_released_while_the_operation_computes() {
    for in_branch in [false, true] {
        let pipe = Mutex::new(Pipe::default());
        let acquisitions = Cell::new(0);
        let acquisition = AcquisitionState::<Locked, _, _>::releasable(|| {
            acquisitions.set(acquisitions.get() + 1);
            async { PipeGuard(pipe.lock().await) }
        });
        assert!(lock_free_while_computing(&pipe, acquisition, in_branch));
        assert_eq!(acquisitions.get(), 2);
    }
}

/// Releases its views only while `contended` is set, as an acquisition that
/// knows whether others wait for the lock would.
struct WhenContended<'a, S> {
    inner: Pin<Box<S>>,
    contended: &'a Cell<bool>,
}
impl<'view, S: nitori_call::ViewAcquisition<'view>> nitori_call::ViewAcquisition<'view>
    for WhenContended<'_, S>
{
    type Family = S::Family;
    fn poll_acquire(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Lend<<S::Family as ReceiverFamily>::ReceiverView<'view>>> {
        self.get_mut().inner.as_mut().poll_acquire(cx)
    }
    fn should_release(
        self: Pin<&mut Self>,
        _: Pin<&mut <S::Family as ReceiverFamily>::ReceiverView<'view>>,
    ) -> bool {
        self.contended.get()
    }
}

#[test]
fn an_acquisition_decides_at_each_release_point() {
    for contended in [false, true] {
        let pipe = Mutex::new(Pipe::default());
        let acquisitions = Cell::new(0);
        let contended_cell = Cell::new(contended);
        let acquisition = WhenContended {
            inner: Box::pin(AcquisitionState::<Locked, _, _>::releasable(|| {
                acquisitions.set(acquisitions.get() + 1);
                async { PipeGuard(pipe.lock().await) }
            })),
            contended: &contended_cell,
        };
        assert_eq!(
            lock_free_while_computing(&pipe, acquisition, true),
            contended
        );
        assert_eq!(acquisitions.get(), 1 + usize::from(contended));
    }
}

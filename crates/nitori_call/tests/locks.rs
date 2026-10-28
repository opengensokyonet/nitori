//! Locks as executors, and `drive_composed` reaching a part of a locked value.
#![feature(impl_trait_in_assoc_type, yield_expr)]
use bytes::Buf;
use nitori_call::{
    AcquisitionState, Compose, Guarded, Lock, LockAcquisition, Locked, ReceiverAccess,
    ReceiverFamily, call, drive_composed,
};
use nitori_io::{calls::WriteAllError, prelude::*};
use std::{
    cell::Cell,
    collections::HashMap,
    future::{Future, poll_fn},
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Waits until `open` is set, then writes `message`.
#[call(sync)]
async fn write_when_open<A: ReceiverAccess<Family: Write>>(
    io: &mut A,
    open: &Cell<bool>,
    message: &[u8],
) -> Result<(), WriteAllError<<A::Family as Write>::Error>> {
    poll_fn(|_| match open.get() {
        true => Poll::Ready(()),
        false => Poll::Pending,
    })
    .await;
    io.write_all(message).await
}

/// Writes, computes without the view, and writes again.
#[call(sync)]
async fn write_compute_write<A: ReceiverAccess<Family: Write>>(
    io: &mut A,
    during: &dyn Fn(),
) -> Result<(), WriteAllError<<A::Family as Write>::Error>> {
    io.write_all(&b"a"[..]).await?;
    io.release_view().await;
    during();
    io.write_all(&b"b"[..]).await
}

#[test]
fn operations_run_on_the_value_behind_a_std_mutex() {
    let output = std::sync::Mutex::new(Vec::new());
    (&output).sync_write_all_unpin(&b"hi"[..]).unwrap();
    assert_eq!(*output.lock().unwrap(), b"hi");
}

#[test]
fn the_lock_is_released_at_release_points() {
    let output = std::sync::Mutex::new(Vec::new());
    let free = || assert!(output.try_lock().is_ok(), "locked while computing");
    (&output).sync_write_compute_write_unpin(&free).unwrap();
    assert_eq!(*output.lock().unwrap(), b"ab");
}

#[test]
#[should_panic = "the lock is poisoned"]
fn a_poisoned_std_mutex_panics() {
    let output = std::sync::Mutex::new(Vec::<u8>::new());
    let _ = std::panic::catch_unwind(|| {
        let _guard = output.lock().unwrap();
        panic!("poison");
    });
    let _ = (&output).sync_write_all_unpin(&b"x"[..]);
}

/// A fixed acquired view stays fixed when composed: the lock is kept
/// through the release point.
#[test]
fn composing_keeps_whether_the_view_is_releasable() {
    let output = std::sync::Mutex::new(Vec::new());
    let acquisition = AcquisitionState::<Locked<std::sync::Mutex<Vec<u8>>>, _, _>::new(|| {
        std::future::ready(output.lock().unwrap())
    });
    let held = || assert!(output.try_lock().is_err(), "a fixed view is kept");
    let mut write = pin!(drive_composed(
        acquisition,
        Guarded,
        WriteComputeWrite::new(&held)
    ));
    assert!(matches!(poll_once(write.as_mut()), Poll::Ready(Ok(()))));
    assert_eq!(*output.lock().unwrap(), b"ab");
}

/// The streams of a connection, reached by id through one lock.
struct Connection {
    streams: HashMap<u32, Vec<u8>>,
}
/// Composes the connection's guard into one stream's view; a stream that
/// was removed is a closed view, which its capability reports.
struct StreamOf(u32);
struct Stream;
struct StreamView<'v>(Option<&'v mut Vec<u8>>);
impl ReceiverFamily for Stream {
    type ReceiverView<'v> = StreamView<'v>;
}
impl<L: Lock<Value = Connection>> Compose<Locked<L>> for StreamOf {
    type Family = Stream;
    fn compose<'s, 'p>(self: Pin<&'s mut Self>, guard: Pin<&'s mut L::Guard<'p>>) -> StreamView<'s>
    where
        Locked<L>: 'p,
        'p: 's,
    {
        let id = self.0;
        StreamView(guard.get_mut().streams.get_mut(&id))
    }
}
#[derive(Debug, PartialEq)]
struct Closed;
impl Write for Stream {
    type Error = Closed;
    fn poll_write<'v, I: Buf + ?Sized>(
        view: Pin<&mut StreamView<'v>>,
        _: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<usize, Closed>>
    where
        Self: 'v,
    {
        let Some(stream) = view.get_mut().0.as_deref_mut() else {
            return Poll::Ready(Err(Closed));
        };
        let count = input.remaining();
        while input.has_remaining() {
            let chunk = input.chunk();
            stream.extend_from_slice(chunk);
            let length = chunk.len();
            input.advance(length);
        }
        Poll::Ready(Ok(count))
    }
}

#[test]
fn a_composition_reaches_one_part_of_the_locked_value() {
    let connection = std::sync::Mutex::new(Connection {
        streams: HashMap::from([(1, Vec::new()), (2, Vec::new())]),
    });
    let open = Cell::new(true);
    let write = |id, message: &'static [u8]| {
        let mut write = pin!(drive_composed(
            LockAcquisition::new(&connection),
            StreamOf(id),
            WriteWhenOpen::new(&open, message),
        ));
        match poll_once(write.as_mut()) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("memory writes complete at once"),
        }
    };
    write(1, b"one").unwrap();
    connection.lock().unwrap().streams.remove(&2);
    assert_eq!(write(2, b"two").unwrap_err().host_error(), Some(&Closed));
    assert_eq!(connection.lock().unwrap().streams[&1], b"one");
}

#[cfg(feature = "tokio-sync")]
mod tokio_sync {
    use super::*;
    use nitori_io::calls::ReadArrayError;
    use std::collections::VecDeque;
    use tokio::sync::Mutex;

    #[test]
    fn the_lock_is_free_while_the_operation_waits_for_something_else() {
        let output = Mutex::new(Vec::new());
        let open = Cell::new(false);
        let mut write = pin!((&output).write_when_open_unpin(&open, b"PING"));
        assert!(poll_once(write.as_mut()).is_pending());
        assert!(output.try_lock().is_ok());
        open.set(true);
        assert!(matches!(poll_once(write.as_mut()), Poll::Ready(Ok(()))));
        assert_eq!(*output.try_lock().unwrap(), b"PING");
    }

    #[test]
    fn the_lock_is_released_at_release_points() {
        let output = Mutex::new(Vec::new());
        let free = || assert!(output.try_lock().is_ok(), "locked while computing");
        let mut write = pin!((&output).write_compute_write_unpin(&free));
        assert!(matches!(poll_once(write.as_mut()), Poll::Ready(Ok(()))));
        assert_eq!(*output.try_lock().unwrap(), b"ab");
    }

    /// Reads one byte, using the value only on polls while `wanted` is set.
    #[call]
    async fn read_when_wanted<A: ReceiverAccess<Family: Read>>(
        io: &mut A,
        wanted: &Cell<bool>,
    ) -> Result<[u8; 1], ReadArrayError<<A::Family as Read>::Error>> {
        poll_fn(|_| match wanted.get() {
            true => Poll::Ready(()),
            false => Poll::Pending,
        })
        .await;
        let mut read = pin!(io.read_array::<1>());
        poll_fn(|cx| match wanted.get() {
            true => read.as_mut().poll(cx),
            false => Poll::Pending,
        })
        .await
    }

    #[test]
    fn a_lock_granted_while_unneeded_is_released() {
        let input = Mutex::new(VecDeque::from(*b"z"));
        let wanted = Cell::new(true);
        let holder = input.try_lock().unwrap();
        let mut read = pin!((&input).read_when_wanted_unpin(&wanted));
        // The operation asks while the lock is held: the acquisition queues.
        assert!(poll_once(read.as_mut()).is_pending());
        // It stops needing the value; the lock is granted to the queued
        // acquisition and released by the next poll.
        wanted.set(false);
        drop(holder);
        assert!(poll_once(read.as_mut()).is_pending());
        assert!(input.try_lock().is_ok());
        wanted.set(true);
        assert!(matches!(poll_once(read.as_mut()), Poll::Ready(Ok(value)) if value == *b"z"));
    }
}

#[cfg(feature = "futures-lock")]
mod futures_lock {
    use super::*;
    use futures_util::lock::Mutex;

    #[test]
    fn the_lock_is_free_while_the_operation_waits_for_something_else() {
        let output = Mutex::new(Vec::new());
        let open = Cell::new(false);
        let mut write = pin!((&output).write_when_open_unpin(&open, b"PING"));
        assert!(poll_once(write.as_mut()).is_pending());
        assert!(output.try_lock().is_some());
        open.set(true);
        assert!(matches!(poll_once(write.as_mut()), Poll::Ready(Ok(()))));
        assert_eq!(*output.try_lock().unwrap(), b"PING");
    }
}

/// Writes `first` to the first receiver, then `second` to the second,
/// checking `between` after the first write.
struct WriteBoth<'a> {
    first: &'a [u8],
    second: &'a [u8],
    between: &'a dyn Fn(),
}
impl<A: nitori_call::Receivers, B: nitori_call::Receivers> nitori_call::Operation<(A, B)>
    for WriteBoth<'_>
{
    type Output = ();
    type Yield = std::convert::Infallible;
}
impl<'s, 'a, A, B> nitori_call::Start<'s, (A, B)> for WriteBoth<'a>
where
    A: ReceiverAccess<Family: Write>,
    B: ReceiverAccess<Family: Write>,
{
    type Fut = impl Future<Output = ()>;
    fn start(
        self,
        (mut a, mut b): (A, B),
        _: nitori_call::Emitter<'s, std::convert::Infallible>,
    ) -> Self::Fut {
        async move {
            let _ = a.write_all(self.first).await;
            (self.between)();
            let _ = b.write_all(self.second).await;
        }
    }
}

#[test]
fn two_locks_are_each_taken_when_their_slot_asks() {
    let first = std::sync::Mutex::new(Vec::new());
    let second = std::sync::Mutex::new(Vec::new());
    let between = || {
        assert!(
            first.try_lock().is_err(),
            "the first view is held for the poll"
        );
        assert!(
            second.try_lock().is_ok(),
            "the second lock is not taken yet"
        );
    };
    let mut write = pin!(drive_composed(
        (LockAcquisition::new(&first), LockAcquisition::new(&second)),
        (Guarded, Guarded),
        WriteBoth {
            first: b"a",
            second: b"b",
            between: &between,
        },
    ));
    assert!(poll_once(write.as_mut()).is_ready());
    assert_eq!(*first.lock().unwrap(), b"a");
    assert_eq!(*second.lock().unwrap(), b"b");
}

#[test]
fn a_tuple_of_receivers_lends_a_view_of_each_on_every_poll() {
    let mut outputs = pin!((Vec::new(), Vec::new()));
    let operation = WriteBoth {
        first: b"a",
        second: b"b",
        between: &|| {},
    };
    let mut write = pin!(nitori_call::CallExecutor::execute(
        outputs.as_mut(),
        operation
    ));
    assert!(poll_once(write.as_mut()).is_ready());
    assert_eq!(outputs.0, b"a");
    assert_eq!(outputs.1, b"b");
}

/// Bytes behind a `RefCell`, borrowed for each acquired view.
struct Ledger;
impl ReceiverFamily for Ledger {
    type ReceiverView<'v> = std::cell::RefMut<'v, Vec<u8>>;
}
impl Write for Ledger {
    type Error = std::convert::Infallible;
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let count = input.remaining();
        host.get_mut()
            .extend_from_slice(&input.copy_to_bytes(count));
        Poll::Ready(Ok(count))
    }
}

#[test]
fn a_constructed_receiver_and_an_acquired_one() {
    let mut output = Vec::new();
    let ledger = std::cell::RefCell::new(Vec::new());
    let acquisition =
        AcquisitionState::<Ledger, _, _>::new(|| std::future::ready(ledger.borrow_mut()));
    let between = || {
        assert!(
            ledger.try_borrow_mut().is_ok(),
            "not acquired before it is asked for"
        )
    };
    {
        let mut write = pin!(nitori_call::drive(
            (
                nitori_call::Constructed::new(Pin::new(&mut output)),
                acquisition
            ),
            WriteBoth {
                first: b"a",
                second: b"b",
                between: &between,
            },
        ));
        assert!(poll_once(write.as_mut()).is_ready());
        assert!(
            ledger.try_borrow_mut().is_ok(),
            "released at the end of the poll"
        );
    }
    assert_eq!(output, b"a");
    assert_eq!(*ledger.borrow(), b"b");
}

/// Writes one byte to each of three receivers.
struct WriteThree;
impl<A: nitori_call::Receivers, B: nitori_call::Receivers, C: nitori_call::Receivers>
    nitori_call::Operation<(A, B, C)> for WriteThree
{
    type Output = ();
    type Yield = std::convert::Infallible;
}
impl<'s, A, B, C> nitori_call::Start<'s, (A, B, C)> for WriteThree
where
    A: ReceiverAccess<Family: Write>,
    B: ReceiverAccess<Family: Write>,
    C: ReceiverAccess<Family: Write>,
{
    type Fut = impl Future<Output = ()>;
    fn start(
        self,
        (mut a, mut b, mut c): (A, B, C),
        _: nitori_call::Emitter<'s, std::convert::Infallible>,
    ) -> Self::Fut {
        async move {
            let _ = c.write_all(&b"c"[..]).await;
            let _ = a.write_all(&b"a"[..]).await;
            let _ = b.write_all(&b"b"[..]).await;
        }
    }
}

#[test]
fn three_locks_driven_together() {
    let locks = [(); 3].map(|_| std::sync::Mutex::new(Vec::new()));
    let mut write = pin!(drive_composed(
        (
            LockAcquisition::new(&locks[0]),
            LockAcquisition::new(&locks[1]),
            LockAcquisition::new(&locks[2]),
        ),
        (Guarded, Guarded, Guarded),
        WriteThree,
    ));
    assert!(poll_once(write.as_mut()).is_ready());
    let written = locks.each_ref().map(|lock| lock.lock().unwrap().clone());
    assert_eq!(written, [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
}

/// Copies `length` bytes, one at a time, from `source` to `sink`.
#[call(sync)]
async fn copy_bytes<F, G>(
    source: &mut impl ReceiverAccess<Family = F>,
    sink: &mut impl ReceiverAccess<Family = G>,
    length: usize,
) -> usize
where
    F: Read,
    G: Write,
{
    let mut copied = 0;
    while copied < length {
        let Ok([byte]) = source.read_array::<1>().await else {
            break;
        };
        if sink.write_all(&[byte][..]).await.is_err() {
            break;
        }
        copied += 1;
    }
    copied
}

/// Copies through `copy_bytes` as a sub-operation, emitting each count.
#[call(yields = usize)]
async fn copy_in_steps<A: ReceiverAccess<Family: Read>, B: ReceiverAccess<Family: Write>>(
    source: &mut A,
    sink: &mut B,
    steps: usize,
) -> usize {
    let mut total = 0;
    for _ in 0..steps {
        let copied = copy_bytes(source, sink, 1).await;
        total += copied;
        yield total;
    }
    total
}

#[test]
fn a_call_over_two_receivers_runs_on_a_tuple_of_them() {
    let mut hosts = pin!((b"abc".as_slice(), Vec::new()));
    assert_eq!(hosts.as_mut().sync_copy_bytes(2), 2);
    assert_eq!(hosts.0, b"c");
    assert_eq!(hosts.1, b"ab");
}

#[test]
fn a_call_over_two_receivers_runs_on_two_locks() {
    let source = std::sync::Mutex::new(std::collections::VecDeque::from(b"xyz".to_vec()));
    let sink = std::sync::Mutex::new(Vec::new());
    let mut copy = pin!(drive_composed(
        (LockAcquisition::new(&source), LockAcquisition::new(&sink)),
        (Guarded, Guarded),
        CopyBytes::new(3),
    ));
    assert_eq!(poll_once(copy.as_mut()), Poll::Ready(3));
    assert_eq!(*sink.lock().unwrap(), b"xyz");
}

#[test]
fn a_call_over_two_receivers_emits_and_calls_another() {
    use futures_core::Stream;
    let mut hosts = pin!((b"ab".as_slice(), Vec::new()));
    let mut events = Vec::new();
    {
        let mut steps = pin!(hosts.as_mut().copy_in_steps(2));
        let mut cx = Context::from_waker(Waker::noop());
        while let Poll::Ready(Some(event)) = steps.as_mut().poll_next(&mut cx) {
            events.push(event);
        }
    }
    use std::ops::ControlFlow::{Break, Continue};
    assert_eq!(events, [Continue(1), Continue(2), Break(2)]);
    assert_eq!(hosts.1, b"ab");
}

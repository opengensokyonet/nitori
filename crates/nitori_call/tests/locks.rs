//! Locks as executors, and `drive_composed` reaching a part of a locked value.
#![feature(impl_trait_in_assoc_type)]
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
) -> Result<usize, WriteAllError<<A::Family as Write>::Error>> {
    poll_fn(|_| match open.get() {
        true => Poll::Ready(()),
        false => Poll::Pending,
    })
    .await;
    io.write_all(message).await.result
}

/// Writes, computes without the view, and writes again.
#[call(sync)]
async fn write_compute_write<A: ReceiverAccess<Family: Write>>(
    io: &mut A,
    during: &dyn Fn(),
) -> Result<usize, WriteAllError<<A::Family as Write>::Error>> {
    io.write_all(&b"a"[..]).await.result?;
    io.release_view().await;
    during();
    io.write_all(&b"b"[..]).await.result
}

#[test]
fn operations_run_on_the_value_behind_a_std_mutex() {
    let output = std::sync::Mutex::new(Vec::new());
    assert_eq!(
        (&output).sync_write_all_unpin(&b"hi"[..]).result.unwrap(),
        2
    );
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
    assert!(matches!(poll_once(write.as_mut()), Poll::Ready(Ok(value)) if value == 1));
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
    assert_eq!(write(1, b"one").unwrap(), 3);
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
        assert!(matches!(poll_once(write.as_mut()), Poll::Ready(Ok(value)) if value == 4));
        assert_eq!(*output.try_lock().unwrap(), b"PING");
    }

    #[test]
    fn the_lock_is_released_at_release_points() {
        let output = Mutex::new(Vec::new());
        let free = || assert!(output.try_lock().is_ok(), "locked while computing");
        let mut write = pin!((&output).write_compute_write_unpin(&free));
        assert!(matches!(poll_once(write.as_mut()), Poll::Ready(Ok(value)) if value == 1));
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
        assert!(matches!(poll_once(write.as_mut()), Poll::Ready(Ok(value)) if value == 4));
        assert_eq!(*output.try_lock().unwrap(), b"PING");
    }
}

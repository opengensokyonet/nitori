//! Byte sources and sinks, locks, and how ordinary async reads and writes
//! frames through them.
use crate::{
    fields::Fields,
    workload::{FrameHeader, Kind, Protocol, data_checksum},
};
use bytes::{Buf, BytesMut};
use std::{
    future::{Future, poll_fn},
    io,
    ops::DerefMut,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll, ready},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, ReadBuf};

pub fn byte_sum(bytes: &[u8]) -> u64 {
    bytes.iter().map(|&b| b as u64).sum()
}

pub fn bump(counter: &AtomicUsize) {
    counter.fetch_add(1, Ordering::Relaxed);
}

// Sources and sinks.

/// A source whose readable bytes sit in a buffer that the reader can visit in
/// place.
pub trait ByteSource {
    fn buffer(&mut self) -> &mut BytesMut;
    /// Make more bytes available, or report the end with `Ready(Ok(false))`.
    /// Called only when the buffer is empty.
    fn poll_more(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<bool>>;
    /// Called after bytes were removed from the front of the buffer.
    fn consumed(&mut self) {}

    /// Visit up to `maximum` bytes in place and consume them. `Ok(0)` means
    /// the end of the source.
    fn poll_consume<F: FnMut(&[u8]) + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        maximum: usize,
        visit: &mut F,
    ) -> Poll<io::Result<usize>> {
        if self.buffer().is_empty() && !ready!(self.poll_more(cx))? {
            return Poll::Ready(Ok(0));
        }
        let buffer = self.buffer();
        let count = maximum.min(buffer.len());
        visit(&buffer[..count]);
        buffer.advance(count);
        self.consumed();
        Poll::Ready(Ok(count))
    }
}

/// A sink that accepts bytes into a buffer of its own.
pub trait ByteSink {
    /// Accept a prefix of `input`, or wait for room.
    fn poll_write<I: Buf + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>>;
    /// Push out what was accepted.
    fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
}

/// Read from an async reader into the spare capacity of a `BytesMut`.
pub fn poll_read_into<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
    cx: &mut Context<'_>,
    destination: &mut BytesMut,
) -> Poll<io::Result<usize>> {
    use bytes::BufMut;
    let chunk = destination.chunk_mut();
    // SAFETY: ReadBuf only writes initialized bytes into the uninitialized slice.
    let mut buffer = ReadBuf::uninit(unsafe { chunk.as_uninit_slice_mut() });
    ready!(Pin::new(reader).poll_read(cx, &mut buffer))?;
    let count = buffer.filled().len();
    // SAFETY: `count` bytes were initialized by the read.
    unsafe { destination.advance_mut(count) };
    Poll::Ready(Ok(count))
}

/// Selects the part of a locked value that one reader or writer uses: the
/// whole value, or one stream of a connection.
pub trait Project<V>: Copy + Send + Sync + Unpin + 'static {
    type Target: ?Sized;
    fn project<'v>(&self, value: &'v mut V) -> &'v mut Self::Target;
}
#[derive(Clone, Copy)]
pub struct Whole;
impl<V> Project<V> for Whole {
    type Target = V;
    fn project<'v>(&self, value: &'v mut V) -> &'v mut V {
        value
    }
}

// Ordinary async.

/// What ordinary async reads through: a source owned by the reader, or one
/// behind a lock.
pub trait PollConsume {
    fn poll_consume<F: FnMut(&[u8]) + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        maximum: usize,
        visit: &mut F,
    ) -> Poll<io::Result<usize>>;
}
impl<S: ByteSource> PollConsume for S {
    fn poll_consume<F: FnMut(&[u8]) + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        maximum: usize,
        visit: &mut F,
    ) -> Poll<io::Result<usize>> {
        ByteSource::poll_consume(self, cx, maximum, visit)
    }
}

/// What ordinary async writes through.
pub trait PollWrite {
    fn poll_write<I: Buf + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>>;
}
impl<S: ByteSink> PollWrite for S {
    fn poll_write<I: Buf + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>> {
        ByteSink::poll_write(self, cx, input)
    }
}

/// A part of a value behind a lock that ordinary async locks on each poll;
/// the lock is released before returning, so it is never held while waiting.
pub struct LockPerPoll<'a, L: Lock, P> {
    attempt: PollLock<'a, L>,
    project: P,
    locks: &'a AtomicUsize,
}
impl<'a, L: Lock, P> LockPerPoll<'a, L, P> {
    pub fn new(lock: &'a L, project: P, locks: &'a AtomicUsize) -> Self {
        Self {
            attempt: PollLock::new(lock),
            project,
            locks,
        }
    }
}
impl<L: Lock, P: Project<L::Value>> PollConsume for LockPerPoll<'_, L, P>
where
    P::Target: ByteSource,
{
    fn poll_consume<F: FnMut(&[u8]) + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        maximum: usize,
        visit: &mut F,
    ) -> Poll<io::Result<usize>> {
        let mut value = ready!(self.attempt.poll_lock(cx));
        bump(self.locks);
        ByteSource::poll_consume(self.project.project(&mut value), cx, maximum, visit)
    }
}
impl<L: Lock, P: Project<L::Value>> PollWrite for LockPerPoll<'_, L, P>
where
    P::Target: ByteSink,
{
    fn poll_write<I: Buf + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>> {
        let mut value = ready!(self.attempt.poll_lock(cx));
        bump(self.locks);
        ByteSink::poll_write(self.project.project(&mut value), cx, input)
    }
}

/// Visit `length` bytes, failing at the end of the source.
async fn consume_exact<S: PollConsume + ?Sized>(
    source: &mut S,
    length: usize,
    mut visit: impl FnMut(&[u8]),
) -> io::Result<()> {
    let mut remaining = length;
    while remaining > 0 {
        let count = poll_fn(|cx| source.poll_consume(cx, remaining, &mut visit)).await?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        remaining -= count;
    }
    Ok(())
}

/// Read one frame with ordinary async and return its checksum, or `None` at
/// the end of the source. A field section is copied into `block` and decoded
/// into `fields`; DATA is visited in place.
pub async fn plain_read_frame<P: Protocol, S: PollConsume + ?Sized>(
    source: &mut S,
    block: &mut Vec<u8>,
    fields: &mut Fields,
) -> io::Result<Option<u64>> {
    let mut header = [0u8; 16];
    let mut filled = 0;
    loop {
        let needed = P::header_needed(&header[..filled]);
        if needed == 0 {
            break;
        }
        let count = poll_fn(|cx| {
            source.poll_consume(cx, needed, &mut |bytes: &[u8]| {
                header[filled..filled + bytes.len()].copy_from_slice(bytes)
            })
        })
        .await?;
        if count == 0 {
            return if filled == 0 {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        filled += count;
    }
    let FrameHeader { kind, length } = P::parse_header(&header[..filled]);
    Ok(Some(match kind {
        Kind::Headers => {
            block.clear();
            consume_exact(source, length, |bytes| block.extend_from_slice(bytes)).await?;
            P::decode_fields(block, fields)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
            fields.checksum()
        }
        Kind::Data => {
            let mut checksum = 0;
            consume_exact(source, length, |bytes| checksum += data_checksum(bytes)).await?;
            checksum
        }
    }))
}

/// Write all of `input` with ordinary async.
pub async fn plain_write_all<S: PollWrite + ?Sized>(sink: &mut S, input: &[u8]) -> io::Result<()> {
    let mut input = input;
    while input.has_remaining() {
        poll_fn(|cx| sink.poll_write(cx, &mut input)).await?;
    }
    Ok(())
}

// Locks.

/// The locks a shared resource can sit behind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LockKind {
    /// `tokio::sync::Mutex`: async, first in first out.
    Fair,
    /// `futures_util::lock::Mutex`: async, no fairness guarantee.
    Unfair,
    /// `std::sync::Mutex`: blocks the thread, no fairness guarantee. A guard
    /// must not be held across an await.
    Blocking,
}
pub const LOCK_KINDS: [LockKind; 3] = [LockKind::Fair, LockKind::Unfair, LockKind::Blocking];

/// A mutex polled from a poll function.
pub trait Lock: Send + Sync + 'static {
    type Value;
    type Guard<'a>: DerefMut<Target = Self::Value> + Unpin
    where
        Self: 'a;
    /// The state of a lock attempt that has to wait.
    type Waiting<'a>: Send + Unpin
    where
        Self: 'a;
    fn new(value: Self::Value) -> Self;
    fn poll_lock<'a>(
        &'a self,
        waiting: &mut Option<Self::Waiting<'a>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Guard<'a>>;
}

impl<T: Send + 'static> Lock for tokio::sync::Mutex<T> {
    type Value = T;
    type Guard<'a> = tokio::sync::MutexGuard<'a, T>;
    type Waiting<'a> = Pin<Box<dyn Future<Output = tokio::sync::MutexGuard<'a, T>> + Send + 'a>>;
    fn new(value: T) -> Self {
        tokio::sync::Mutex::new(value)
    }
    /// Tries the lock first; when it is held, keeps the place in the queue
    /// across polls. Like `lock()`, it consumes the task's cooperative budget.
    fn poll_lock<'a>(
        &'a self,
        waiting: &mut Option<Self::Waiting<'a>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Guard<'a>> {
        let acquiring = match waiting {
            Some(acquiring) => acquiring,
            None => {
                let budget = ready!(tokio::task::coop::poll_proceed(cx));
                if let Ok(guard) = self.try_lock() {
                    budget.made_progress();
                    return Poll::Ready(guard);
                }
                waiting.insert(Box::pin(self.lock()))
            }
        };
        let guard = ready!(acquiring.as_mut().poll(cx));
        *waiting = None;
        Poll::Ready(guard)
    }
}

impl<T: Send + 'static> Lock for futures_util::lock::Mutex<T> {
    type Value = T;
    type Guard<'a> = futures_util::lock::MutexGuard<'a, T>;
    type Waiting<'a> = futures_util::lock::MutexLockFuture<'a, T>;
    fn new(value: T) -> Self {
        futures_util::lock::Mutex::new(value)
    }
    fn poll_lock<'a>(
        &'a self,
        waiting: &mut Option<Self::Waiting<'a>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Guard<'a>> {
        let acquiring = waiting.get_or_insert_with(|| self.lock());
        let guard = ready!(Pin::new(acquiring).poll(cx));
        *waiting = None;
        Poll::Ready(guard)
    }
}

impl<T: Send + 'static> Lock for std::sync::Mutex<T> {
    type Value = T;
    type Guard<'a> = std::sync::MutexGuard<'a, T>;
    type Waiting<'a> = ();
    fn new(value: T) -> Self {
        std::sync::Mutex::new(value)
    }
    fn poll_lock<'a>(&'a self, _: &mut Option<()>, _: &mut Context<'_>) -> Poll<Self::Guard<'a>> {
        Poll::Ready(self.lock().unwrap())
    }
}

/// Runs `run` with the lock type of `kind` around `T`.
pub trait PerLock<T> {
    type Output;
    fn run<L: Lock<Value = T>>(self) -> Self::Output;
}
pub fn with_lock<T: Send + 'static, P: PerLock<T>>(kind: LockKind, per_lock: P) -> P::Output {
    match kind {
        LockKind::Fair => per_lock.run::<tokio::sync::Mutex<T>>(),
        LockKind::Unfair => per_lock.run::<futures_util::lock::Mutex<T>>(),
        LockKind::Blocking => per_lock.run::<std::sync::Mutex<T>>(),
    }
}

/// A lock attempt that may span several polls.
pub struct PollLock<'a, L: Lock> {
    lock: &'a L,
    waiting: Option<L::Waiting<'a>>,
}
impl<'a, L: Lock> PollLock<'a, L> {
    pub fn new(lock: &'a L) -> Self {
        Self {
            lock,
            waiting: None,
        }
    }
    pub fn poll_lock(&mut self, cx: &mut Context<'_>) -> Poll<L::Guard<'a>> {
        self.lock.poll_lock(&mut self.waiting, cx)
    }
}

pub async fn lock<L: Lock>(lock: &L) -> L::Guard<'_> {
    let mut attempt = PollLock::new(lock);
    poll_fn(|cx| attempt.poll_lock(cx)).await
}

/// A lock attempt that records how long each acquisition took.
pub struct TimedLock<'a, L: Lock> {
    attempt: PollLock<'a, L>,
    started: Option<Instant>,
}
impl<'a, L: Lock> TimedLock<'a, L> {
    pub fn new(lock: &'a L) -> Self {
        Self {
            attempt: PollLock::new(lock),
            started: None,
        }
    }
    pub fn poll_lock(
        &mut self,
        cx: &mut Context<'_>,
        waits: &mut Vec<Duration>,
    ) -> Poll<L::Guard<'a>> {
        let start = *self.started.get_or_insert_with(Instant::now);
        let guard = ready!(self.attempt.poll_lock(cx));
        waits.push(start.elapsed());
        self.started = None;
        Poll::Ready(guard)
    }
}

/// How often [`probe`] locks.
pub const PROBE_INTERVAL: Duration = Duration::from_micros(20);

/// Lock every [`PROBE_INTERVAL`] until `stop`, recording each wait for the
/// lock. Between locks it only yields to the scheduler.
pub async fn probe<L: Lock>(lock: Arc<L>, stop: Arc<AtomicBool>) -> Vec<Duration> {
    let mut waits = Vec::new();
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        tokio::task::yield_now().await;
        if Instant::now() < next {
            continue;
        }
        let start = Instant::now();
        let mut attempt = TimedLock::new(&*lock);
        drop(poll_fn(|cx| attempt.poll_lock(cx, &mut waits)).await);
        next = start + PROBE_INTERVAL;
    }
    waits
}

// Runs.

#[derive(Default)]
pub struct Counters {
    pub locks: AtomicUsize,
    pub polls: AtomicUsize,
    /// Peak capacity of a codec's own buffer.
    pub buffer: AtomicUsize,
}
impl Counters {
    pub fn buffer_at_least(&self, capacity: usize) {
        self.buffer.fetch_max(capacity, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct Sample {
    pub elapsed: Duration,
    pub locks: usize,
    pub polls: usize,
    pub buffer: usize,
    /// How long the other user of the shared resource waited for its lock.
    pub waits: Vec<Duration>,
}
impl Sample {
    pub fn new(elapsed: Duration, counters: &Counters, waits: Vec<Duration>) -> Self {
        Self {
            elapsed,
            locks: counters.locks.load(Ordering::Relaxed),
            polls: counters.polls.load(Ordering::Relaxed),
            buffer: counters.buffer.load(Ordering::Relaxed),
            waits,
        }
    }
}

/// Await `future`, counting how often it is polled.
pub async fn counted<F: Future>(polls: &AtomicUsize, future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    poll_fn(|cx| {
        bump(polls);
        future.as_mut().poll(cx)
    })
    .await
}

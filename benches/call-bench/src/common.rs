//! Byte sources and sinks, locks, and how ordinary async reads and writes
//! frames through them.
use crate::{
    fields::Fields,
    workload::{FrameHeader, Kind, Protocol, Work},
};
use bytes::{Buf, BytesMut};
use futures_core::Stream;
use futures_util::{TryStreamExt as _, stream};
use std::{
    future::{Future, poll_fn},
    io,
    ops::{Deref, DerefMut},
    pin::{Pin, pin},
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
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
    /// Called after `count` bytes were removed from the front of the buffer.
    fn consumed(&mut self, _count: usize) {}

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
        self.consumed(count);
        Poll::Ready(Ok(count))
    }

    /// Take up to `maximum` readable bytes out of the buffer without copying.
    /// An empty result means the end of the source.
    fn poll_take(&mut self, cx: &mut Context<'_>, maximum: usize) -> Poll<io::Result<BytesMut>> {
        if self.buffer().is_empty() && !ready!(self.poll_more(cx))? {
            return Poll::Ready(Ok(BytesMut::new()));
        }
        let buffer = self.buffer();
        let count = maximum.min(buffer.len());
        let piece = buffer.split_to(count);
        self.consumed(count);
        Poll::Ready(Ok(piece))
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
    fn poll_take(&mut self, cx: &mut Context<'_>, maximum: usize) -> Poll<io::Result<BytesMut>>;
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
    fn poll_take(&mut self, cx: &mut Context<'_>, maximum: usize) -> Poll<io::Result<BytesMut>> {
        ByteSource::poll_take(self, cx, maximum)
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

/// A value behind a lock that ordinary async locks for each leaf operation;
/// the lock is released before returning, so it is never held while waiting.
/// One poll of the task may lock it several times.
pub struct LockPerLeaf<'a, L: Lock> {
    attempt: PollLock<'a, L>,
}
impl<'a, L: Lock> LockPerLeaf<'a, L> {
    pub fn new(lock: &'a L, stats: &'a LockStats) -> Self {
        Self {
            attempt: PollLock::new(lock, stats),
        }
    }
}
impl<L: Lock> PollConsume for LockPerLeaf<'_, L>
where
    L::Value: ByteSource,
{
    fn poll_consume<F: FnMut(&[u8]) + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        maximum: usize,
        visit: &mut F,
    ) -> Poll<io::Result<usize>> {
        let mut value = ready!(self.attempt.poll_lock(cx));
        ByteSource::poll_consume(&mut *value, cx, maximum, visit)
    }
    fn poll_take(&mut self, cx: &mut Context<'_>, maximum: usize) -> Poll<io::Result<BytesMut>> {
        let mut value = ready!(self.attempt.poll_lock(cx));
        ByteSource::poll_take(&mut *value, cx, maximum)
    }
}
impl<L: Lock> PollWrite for LockPerLeaf<'_, L>
where
    L::Value: ByteSink,
{
    fn poll_write<I: Buf + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>> {
        let mut value = ready!(self.attempt.poll_lock(cx));
        ByteSink::poll_write(&mut *value, cx, input)
    }
}

/// Read one frame's header with ordinary async, and return its kind and a
/// stream of its payload in pieces taken out of the source without copying,
/// or `None` at the end of the source.
pub async fn plain_read_frame<P: Protocol, S: PollConsume + ?Sized>(
    source: &mut S,
) -> io::Result<Option<(Kind, impl Stream<Item = io::Result<BytesMut>>)>> {
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
    let payload = stream::try_unfold((source, length), async |(source, remaining)| {
        if remaining == 0 {
            return Ok(None);
        }
        let piece = poll_fn(|cx| source.poll_take(cx, remaining)).await?;
        if piece.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let left = remaining - piece.len();
        Ok(Some((piece, (source, left))))
    });
    Ok(Some((kind, payload)))
}

/// Read one frame with ordinary async and return its checksum, or `None` at
/// the end of the source. A field section is copied into `block` and decoded
/// into `fields`; each piece of DATA is processed with `work` as it arrives,
/// after the source is released.
pub async fn plain_frame_checksum<P: Protocol, S: PollConsume + ?Sized>(
    source: &mut S,
    block: &mut Vec<u8>,
    fields: &mut Fields,
    work: Work,
) -> io::Result<Option<u64>> {
    let Some((kind, payload)) = plain_read_frame::<P, S>(source).await? else {
        return Ok(None);
    };
    let mut payload = pin!(payload);
    Ok(Some(match kind {
        Kind::Headers => {
            block.clear();
            while let Some(piece) = payload.try_next().await? {
                block.extend_from_slice(&piece);
            }
            P::decode_fields(block, fields)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
            fields.checksum()
        }
        Kind::Data => {
            let mut checksum = 0;
            while let Some(piece) = payload.try_next().await? {
                checksum += work.read(&piece);
            }
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
    type Value: Progress;
    type Guard<'a>: DerefMut<Target = Self::Value> + Unpin
    where
        Self: 'a;
    /// The state of a lock attempt that has to wait.
    type Waiting<'a>: Send + Unpin
    where
        Self: 'a;
    fn new(value: Self::Value) -> Self;
    /// Lock, setting `queued` when the lock was held by someone else.
    fn poll_lock<'a>(
        &'a self,
        waiting: &mut Option<Self::Waiting<'a>>,
        queued: &mut bool,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Guard<'a>>;
}

impl<T: Progress + Send + 'static> Lock for tokio::sync::Mutex<T> {
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
        queued: &mut bool,
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
                *queued = true;
                waiting.insert(Box::pin(self.lock()))
            }
        };
        let guard = ready!(acquiring.as_mut().poll(cx));
        *waiting = None;
        Poll::Ready(guard)
    }
}

impl<T: Progress + Send + 'static> Lock for futures_util::lock::Mutex<T> {
    type Value = T;
    type Guard<'a> = futures_util::lock::MutexGuard<'a, T>;
    type Waiting<'a> = futures_util::lock::MutexLockFuture<'a, T>;
    fn new(value: T) -> Self {
        futures_util::lock::Mutex::new(value)
    }
    fn poll_lock<'a>(
        &'a self,
        waiting: &mut Option<Self::Waiting<'a>>,
        queued: &mut bool,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Guard<'a>> {
        let acquiring = match waiting {
            Some(acquiring) => acquiring,
            None => {
                if let Some(guard) = self.try_lock() {
                    return Poll::Ready(guard);
                }
                *queued = true;
                waiting.insert(self.lock())
            }
        };
        let guard = ready!(Pin::new(acquiring).poll(cx));
        *waiting = None;
        Poll::Ready(guard)
    }
}

impl<T: Progress + Send + 'static> Lock for std::sync::Mutex<T> {
    type Value = T;
    type Guard<'a> = std::sync::MutexGuard<'a, T>;
    type Waiting<'a> = ();
    fn new(value: T) -> Self {
        std::sync::Mutex::new(value)
    }
    fn poll_lock<'a>(
        &'a self,
        _: &mut Option<()>,
        queued: &mut bool,
        _: &mut Context<'_>,
    ) -> Poll<Self::Guard<'a>> {
        match self.try_lock() {
            Ok(guard) => Poll::Ready(guard),
            Err(std::sync::TryLockError::WouldBlock) => {
                *queued = true;
                Poll::Ready(self.lock().unwrap())
            }
            Err(std::sync::TryLockError::Poisoned(error)) => panic!("{error}"),
        }
    }
}

/// A count that changes whenever a value behind a lock changes in a way that
/// matters, so that an acquisition that changed nothing can be told apart.
pub trait Progress {
    fn progress(&self) -> u64;
}

/// What one side's acquisitions of a lock did.
pub struct LockStats {
    acquired: AtomicUsize,
    queued: AtomicUsize,
    empty: AtomicUsize,
    /// Nanoseconds held in total; measured only when `timed`.
    held: AtomicU64,
    timed: bool,
}
impl LockStats {
    pub fn new(timed: bool) -> Self {
        Self {
            acquired: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            empty: AtomicUsize::new(0),
            held: AtomicU64::new(0),
            timed,
        }
    }
    pub fn counts(&self) -> LockCounts {
        LockCounts {
            acquired: self.acquired.load(Ordering::Relaxed),
            queued: self.queued.load(Ordering::Relaxed),
            empty: self.empty.load(Ordering::Relaxed),
            held: self
                .timed
                .then(|| Duration::from_nanos(self.held.load(Ordering::Relaxed))),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct LockCounts {
    pub acquired: usize,
    /// Acquisitions that found the lock held by someone else.
    pub queued: usize,
    /// Acquisitions after which the value had not changed.
    pub empty: usize,
    /// Total time held, when measured.
    pub held: Option<Duration>,
}

/// A held lock that records into its side's [`LockStats`] when released.
pub struct Held<'a, L: Lock> {
    guard: L::Guard<'a>,
    stats: &'a LockStats,
    progress: u64,
    since: Option<Instant>,
}
impl<'a, L: Lock> Held<'a, L> {
    fn new(guard: L::Guard<'a>, stats: &'a LockStats, queued: bool) -> Self {
        bump(&stats.acquired);
        if queued {
            bump(&stats.queued);
        }
        Self {
            progress: guard.progress(),
            since: stats.timed.then(Instant::now),
            guard,
            stats,
        }
    }
}
impl<L: Lock> Deref for Held<'_, L> {
    type Target = L::Value;
    fn deref(&self) -> &L::Value {
        &self.guard
    }
}
impl<L: Lock> DerefMut for Held<'_, L> {
    fn deref_mut(&mut self) -> &mut L::Value {
        &mut self.guard
    }
}
impl<L: Lock> Drop for Held<'_, L> {
    fn drop(&mut self) {
        if let Some(since) = self.since {
            let held = since.elapsed().as_nanos() as u64;
            self.stats.held.fetch_add(held, Ordering::Relaxed);
        }
        if self.guard.progress() == self.progress {
            bump(&self.stats.empty);
        }
    }
}

/// Runs `run` with the lock type of `kind` around `T`.
pub trait PerLock<T> {
    type Output;
    fn run<L: Lock<Value = T>>(self) -> Self::Output;
}
pub fn with_lock<T: Progress + Send + 'static, P: PerLock<T>>(
    kind: LockKind,
    per_lock: P,
) -> P::Output {
    match kind {
        LockKind::Fair => per_lock.run::<tokio::sync::Mutex<T>>(),
        LockKind::Unfair => per_lock.run::<futures_util::lock::Mutex<T>>(),
        LockKind::Blocking => per_lock.run::<std::sync::Mutex<T>>(),
    }
}

/// A lock attempt that may span several polls.
pub struct PollLock<'a, L: Lock> {
    lock: &'a L,
    stats: &'a LockStats,
    waiting: Option<L::Waiting<'a>>,
    queued: bool,
}
impl<'a, L: Lock> PollLock<'a, L> {
    pub fn new(lock: &'a L, stats: &'a LockStats) -> Self {
        Self {
            lock,
            stats,
            waiting: None,
            queued: false,
        }
    }
    pub fn poll_lock(&mut self, cx: &mut Context<'_>) -> Poll<Held<'a, L>> {
        let guard = ready!(self.lock.poll_lock(&mut self.waiting, &mut self.queued, cx));
        let queued = std::mem::take(&mut self.queued);
        Poll::Ready(Held::new(guard, self.stats, queued))
    }
    /// Whether an attempt is waiting for the lock.
    pub fn is_pending(&self) -> bool {
        self.queued
    }
}

pub async fn lock<'a, L: Lock>(lock: &'a L, stats: &'a LockStats) -> Held<'a, L> {
    let mut attempt = PollLock::new(lock, stats);
    poll_fn(|cx| attempt.poll_lock(cx)).await
}

/// A lock attempt that records how long each acquisition took.
pub struct TimedLock<'a, L: Lock> {
    attempt: PollLock<'a, L>,
    started: Option<Instant>,
}
impl<'a, L: Lock> TimedLock<'a, L> {
    pub fn new(lock: &'a L, stats: &'a LockStats) -> Self {
        Self {
            attempt: PollLock::new(lock, stats),
            started: None,
        }
    }
    pub fn poll_lock(
        &mut self,
        cx: &mut Context<'_>,
        waits: &mut Vec<Duration>,
    ) -> Poll<Held<'a, L>> {
        let start = *self.started.get_or_insert_with(Instant::now);
        let guard = ready!(self.attempt.poll_lock(cx));
        waits.push(start.elapsed());
        self.started = None;
        Poll::Ready(guard)
    }
}

/// Wait for a wakeup registered while a lock was held: the first poll is
/// pending, the next, after the wakeup, ready. A wakeup that came while the
/// caller was busy has already scheduled the task again, so this saves an
/// acquisition that would only find nothing new.
pub async fn woken() {
    let mut polled = false;
    poll_fn(|_| {
        if std::mem::replace(&mut polled, true) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await
}

/// How fast the source's bytes arrive.
#[derive(Clone, Copy, Debug)]
pub enum Pace {
    /// As fast as the other side takes them.
    Unlimited,
    /// At this many bytes per second, released whenever the timer fires; its
    /// resolution is a millisecond, so they arrive in bursts.
    Rate(u64),
}

/// Releases bytes at a [`Pace`].
pub struct Pacer {
    pace: Pace,
    start: Instant,
    released: u64,
}
impl Pacer {
    pub fn new(pace: Pace) -> Self {
        Self {
            pace,
            start: Instant::now(),
            released: 0,
        }
    }
    /// Wait until `count` more bytes may arrive.
    pub async fn admit(&mut self, count: usize) {
        self.released += count as u64;
        if let Pace::Rate(rate) = self.pace {
            let due = self.start + Duration::from_secs_f64(self.released as f64 / rate as f64);
            if due > Instant::now() {
                tokio::time::sleep_until(due.into()).await;
            }
        }
    }
}

// Runs.

pub struct Counters {
    pub polls: AtomicUsize,
    /// Peak capacity of a codec's own buffer.
    pub buffer: AtomicUsize,
    /// The measured side's locks.
    pub locks: LockStats,
    /// The locks of the other side of the shared resource.
    pub other: LockStats,
}
impl Counters {
    pub fn new(timed: bool) -> Self {
        Self {
            polls: AtomicUsize::new(0),
            buffer: AtomicUsize::new(0),
            locks: LockStats::new(timed),
            other: LockStats::new(timed),
        }
    }
    pub fn buffer_at_least(&self, capacity: usize) {
        self.buffer.fetch_max(capacity, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct Sample {
    pub elapsed: Duration,
    pub polls: usize,
    pub buffer: usize,
    pub locks: LockCounts,
    pub other: LockCounts,
    /// How long the other side waited for its lock.
    pub waits: Vec<Duration>,
}
impl Sample {
    pub fn new(elapsed: Duration, counters: &Counters, waits: Vec<Duration>) -> Self {
        Self {
            elapsed,
            polls: counters.polls.load(Ordering::Relaxed),
            buffer: counters.buffer.load(Ordering::Relaxed),
            locks: counters.locks.counts(),
            other: counters.other.counts(),
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

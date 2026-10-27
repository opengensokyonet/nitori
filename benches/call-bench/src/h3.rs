//! HTTP/3 with QPACK over the stream buffers of a QUIC connection in user
//! space. Each stream's buffer sits behind a lock of its own and one task per
//! stream reads or writes it; a connection task locks only the stream it
//! delivers to or takes from. What the connection task needs to know about a
//! stream without locking it, how far it may deliver and whether there is
//! anything to send, it reads from atomics that the stream updates.
//!
//! Receiving: the connection task delivers packets of up to [`PACKET`] bytes
//! at the input's pace, taking the streams in turn, within each stream's flow
//! control limit. A reader raises the limit once it has consumed the update
//! threshold since the last raise, as a MAX_STREAM_DATA frame would.
//!
//! Sending: each stream may hold [`WINDOW`] bytes; the connection task takes
//! up to [`PACKET`] bytes at a time from the streams that have some, in turn
//! and at the input's pace, and checks the frames after unlocking. A writer
//! waiting for room asks for at most the update threshold, and is woken once
//! that much is free.
//!
//! Holding a lock while waiting would keep the connection task from making
//! progress, so no variant does.
use crate::{
    calls::{ReadAllHostExt as _, Release, Shared, WriteSlotHostExt as _},
    codec::{FrameDecoder, FrameEncoder, Item, Processor},
    common::{
        ByteSink, ByteSource, Counters, Lock, LockKind, LockPerLeaf, Pace, Pacer, PerLock,
        PollLock, Progress, Sample, TimedLock, counted, plain_read_frame, plain_write_all,
        with_lock, woken,
    },
    fields::Fields,
    workload::{Frame, Http3, Message, Work, encode_all, slot_frames, total_checksum},
};
use bytes::{Buf, BytesMut};
use futures_sink::Sink;
use futures_util::task::AtomicWaker;
use std::{
    collections::VecDeque,
    future::poll_fn,
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker, ready},
    time::{Duration, Instant},
};
use tokio::{io::AsyncWrite, runtime::Runtime};
use tokio_util::codec::{Decoder, FramedWrite};

/// The most bytes a stream buffer may hold.
pub const WINDOW: usize = 64 * 1024;
/// Stream bytes per packet.
pub const PACKET: usize = 1350;

fn wake(waker: &mut Option<Waker>) {
    if let Some(waker) = waker.take() {
        waker.wake();
    }
}

#[derive(Clone)]
pub struct Input {
    pub messages: Arc<Vec<Message>>,
    pub streams: usize,
    pub chunk: usize,
    pub pace: Pace,
    /// Yield to the scheduler after each packet received.
    pub trickle: bool,
    /// Bytes a reader consumes before raising its stream's limit; the most
    /// room a waiting writer asks for.
    pub update: usize,
    pub work: Work,
    /// Each stream's frames, encoded.
    pub bytes: Arc<Vec<Vec<u8>>>,
    /// What readers doing `work` compute.
    pub checksum: u64,
    /// What the connection task computes from the frames written.
    pub written: u64,
}
impl Input {
    pub fn new(
        messages: Arc<Vec<Message>>,
        streams: usize,
        chunk: usize,
        pace: Pace,
        trickle: bool,
        update: usize,
        work: Work,
    ) -> Self {
        assert!(
            0 < update && update < WINDOW,
            "the update threshold must be within the window"
        );
        let bytes = (0..streams)
            .map(|slot| encode_all::<Http3>(slot_frames(&messages, slot, streams, chunk)))
            .collect::<Vec<_>>()
            .into();
        let checksum = total_checksum(messages.iter(), work);
        let written = total_checksum(messages.iter(), Work::CHECK);
        Self {
            messages,
            streams,
            chunk,
            pace,
            trickle,
            update,
            work,
            bytes,
            checksum,
            written,
        }
    }
}

/// Spawn one task per stream and add up what they return.
async fn per_stream<F>(streams: usize, task: impl Fn(usize) -> F) -> u64
where
    F: Future<Output = u64> + Send + 'static,
{
    let tasks: Vec<_> = (0..streams)
        .map(|index| tokio::spawn(task(index)))
        .collect();
    let mut total = 0;
    for task in tasks {
        total += task.await.unwrap();
    }
    total
}

/// Poll `find` until it finds something, registering with `waker` and
/// trying once more before waiting, so that no wakeup is missed.
fn poll_find<T>(
    waker: &AtomicWaker,
    cx: &mut Context<'_>,
    mut find: impl FnMut() -> Poll<T>,
) -> Poll<T> {
    if let Poll::Ready(found) = find() {
        return Poll::Ready(found);
    }
    waker.register(cx.waker());
    find()
}

// Receiving.

pub struct RecvStream {
    buffer: BytesMut,
    closed: bool,
    reader: Option<Waker>,
    delivered: usize,
    consumed: usize,
    /// Consumed since the limit was last raised.
    unreported: usize,
    update: usize,
    /// How far the connection task may deliver.
    limit: Arc<AtomicUsize>,
    connection: Arc<AtomicWaker>,
}
impl Progress for RecvStream {
    fn progress(&self) -> u64 {
        (self.delivered + self.consumed) as u64 + self.closed as u64
    }
}
impl ByteSource for RecvStream {
    fn buffer(&mut self) -> &mut BytesMut {
        &mut self.buffer
    }
    fn poll_more(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<bool>> {
        if self.closed {
            return Poll::Ready(Ok(false));
        }
        self.reader = Some(cx.waker().clone());
        Poll::Pending
    }
    fn consumed(&mut self, count: usize) {
        self.consumed += count;
        self.unreported += count;
        if self.unreported >= self.update {
            self.unreported = 0;
            self.limit.store(self.consumed + WINDOW, Ordering::Release);
            self.connection.wake();
        }
    }
}
pub struct RecvConn<L> {
    streams: Vec<L>,
    limits: Vec<Arc<AtomicUsize>>,
    waker: Arc<AtomicWaker>,
}

/// Deliver every stream's bytes, closing each stream with its last packet.
/// Returns the waits for the stream locks.
async fn deliver<L: Lock<Value = RecvStream>>(
    conn: Arc<RecvConn<L>>,
    input: Input,
    counters: Arc<Counters>,
) -> Vec<Duration> {
    let streams = input.streams;
    let mut offsets = vec![0; streams];
    let mut closed = vec![false; streams];
    let mut pacer = Pacer::new(input.pace);
    let mut waits = Vec::new();
    let mut next = 0;
    loop {
        // The next open stream in turn that has room for its next bytes, or
        // that only needs closing.
        let found = poll_fn(|cx| {
            poll_find(&conn.waker, cx, || {
                let mut open = false;
                for turn in 0..streams {
                    let index = (next + turn) % streams;
                    if closed[index] {
                        continue;
                    }
                    open = true;
                    let left = input.bytes[index].len() - offsets[index];
                    let room = conn.limits[index].load(Ordering::Acquire) - offsets[index];
                    if left == 0 || room > 0 {
                        return Poll::Ready(Some((index, left.min(room).min(PACKET))));
                    }
                }
                if open {
                    Poll::Pending
                } else {
                    Poll::Ready(None)
                }
            })
        })
        .await;
        let Some((index, count)) = found else {
            return waits;
        };
        next = index + 1;
        pacer.admit(count).await;
        let data = &input.bytes[index];
        let end = offsets[index] + count == data.len();
        let mut attempt = TimedLock::new(&conn.streams[index], &counters.other);
        poll_fn(|cx| {
            let mut stream = ready!(attempt.poll_lock(cx, &mut waits));
            stream
                .buffer
                .extend_from_slice(&data[offsets[index]..offsets[index] + count]);
            stream.delivered += count;
            stream.closed |= end;
            wake(&mut stream.reader);
            Poll::Ready(())
        })
        .await;
        offsets[index] += count;
        closed[index] = end;
        if input.trickle {
            tokio::task::yield_now().await;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecvVariant {
    PlainLockPerLeaf,
    /// One call reads every frame, keeping the view until the end of each
    /// poll.
    Call,
    /// The same, releasing the view at every release point.
    CallRelease,
    /// Lock, decode every item that is ready, and process them after
    /// unlocking.
    Codec,
}
pub const RECVS: [RecvVariant; 4] = [
    RecvVariant::PlainLockPerLeaf,
    RecvVariant::Call,
    RecvVariant::CallRelease,
    RecvVariant::Codec,
];

async fn receive<L: Lock<Value = RecvStream>>(
    variant: RecvVariant,
    stream: &L,
    counters: &Counters,
    work: Work,
) -> u64 {
    let (mut block, mut fields) = (Vec::new(), Fields::default());
    match variant {
        RecvVariant::PlainLockPerLeaf => {
            let mut source = LockPerLeaf::new(stream, &counters.locks);
            let mut total = 0;
            while let Some(checksum) =
                plain_read_frame::<Http3, _>(&mut source, &mut block, &mut fields, work)
                    .await
                    .unwrap()
            {
                total += checksum;
            }
            total
        }
        RecvVariant::Call | RecvVariant::CallRelease => {
            let release = match variant {
                RecvVariant::CallRelease => Release::Always,
                _ => Release::Never,
            };
            let mut executor = Shared {
                lock: stream,
                stats: &counters.locks,
                release,
            };
            executor
                .read_all_unpin(Http3, &mut block, &mut fields, work)
                .await
                .unwrap()
        }
        RecvVariant::Codec => {
            let mut codec = FrameDecoder::<Http3>::default();
            let mut processor = Processor::<Http3>::new(work);
            let mut items: Vec<Item> = Vec::new();
            let mut ended = false;
            while !ended {
                let mut attempt = PollLock::new(stream, &counters.locks);
                poll_fn(|cx| {
                    let mut stream = ready!(attempt.poll_lock(cx));
                    let before = stream.buffer.len();
                    codec.decode_ready(&mut stream.buffer, &mut items);
                    let taken = before - stream.buffer.len();
                    if taken > 0 {
                        stream.consumed(taken);
                    }
                    if stream.closed {
                        assert!(stream.buffer.is_empty(), "incomplete frame");
                        ended = true;
                        return Poll::Ready(());
                    }
                    // Ask for a wakeup before unlocking, so that the next
                    // acquisition waits for new bytes.
                    stream.reader = Some(cx.waker().clone());
                    if items.is_empty() {
                        Poll::Pending
                    } else {
                        Poll::Ready(())
                    }
                })
                .await;
                for item in items.drain(..) {
                    processor.add(item);
                }
                if !ended {
                    woken().await;
                }
            }
            processor.checksum
        }
    }
}

struct RunRecv<'r> {
    runtime: &'r Runtime,
    variant: RecvVariant,
    input: Input,
    holds: bool,
}
impl PerLock<RecvStream> for RunRecv<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = RecvStream>>(self) -> Sample {
        let RunRecv {
            runtime,
            variant,
            input,
            holds,
        } = self;
        let (expected, work) = (input.checksum, input.work);
        runtime.block_on(async move {
            let waker = Arc::new(AtomicWaker::new());
            let limits: Vec<_> = (0..input.streams)
                .map(|_| Arc::new(AtomicUsize::new(WINDOW)))
                .collect();
            let streams = limits
                .iter()
                .map(|limit| {
                    L::new(RecvStream {
                        buffer: BytesMut::with_capacity(WINDOW),
                        closed: false,
                        reader: None,
                        delivered: 0,
                        consumed: 0,
                        unreported: 0,
                        update: input.update,
                        limit: limit.clone(),
                        connection: waker.clone(),
                    })
                })
                .collect();
            let conn = Arc::new(RecvConn {
                streams,
                limits,
                waker,
            });
            let counters = Arc::new(Counters::new(holds));
            let start = Instant::now();
            // Joined below; it ends after closing every stream.
            let deliverer = tokio::spawn(deliver(conn.clone(), input.clone(), counters.clone()));
            let checksum = per_stream(input.streams, |index| {
                let (conn, counters) = (conn.clone(), counters.clone());
                async move {
                    counted(
                        &counters.polls,
                        receive(variant, &conn.streams[index], &counters, work),
                    )
                    .await
                }
            })
            .await;
            let elapsed = start.elapsed();
            let waits = deliverer.await.unwrap();
            assert_eq!(checksum, expected, "{variant:?}");
            Sample::new(elapsed, &counters, waits)
        })
    }
}

pub fn run_recv(
    runtime: &Runtime,
    variant: RecvVariant,
    lock: LockKind,
    input: Input,
    holds: bool,
) -> Sample {
    with_lock(
        lock,
        RunRecv {
            runtime,
            variant,
            input,
            holds,
        },
    )
}

// Sending.

/// What the connection task reads from a stream without locking it.
#[derive(Default)]
struct SendState {
    buffered: AtomicUsize,
    closed: AtomicBool,
}
pub struct SendStream {
    buffer: BytesMut,
    writer: Option<Waker>,
    /// The room the waiting writer needs.
    wanted: usize,
    update: usize,
    closed: bool,
    /// Bytes in and out, and the close.
    moved: usize,
    state: Arc<SendState>,
    connection: Arc<AtomicWaker>,
}
impl SendStream {
    fn credit(&self) -> usize {
        WINDOW - self.buffer.len()
    }
    fn wait_for_room(&mut self, cx: &mut Context<'_>, room: usize) {
        self.writer = Some(cx.waker().clone());
        self.wanted = room;
    }
    /// Append bytes, waking the connection task if the stream had none.
    fn append(&mut self, bytes: &[u8]) {
        let had_none = self.buffer.is_empty();
        self.buffer.extend_from_slice(bytes);
        self.moved += bytes.len();
        self.state
            .buffered
            .store(self.buffer.len(), Ordering::Release);
        if had_none {
            self.connection.wake();
        }
    }
    fn close(&mut self) {
        self.closed = true;
        self.moved += 1;
        self.state.closed.store(true, Ordering::Release);
        self.connection.wake();
    }
}
impl Progress for SendStream {
    fn progress(&self) -> u64 {
        self.moved as u64
    }
}
impl ByteSink for SendStream {
    fn poll_write<I: Buf + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>> {
        if !input.has_remaining() {
            return Poll::Ready(Ok(0));
        }
        if self.credit() == 0 {
            let room = self.update.min(input.remaining());
            self.wait_for_room(cx, room);
            return Poll::Pending;
        }
        let accepted = self.credit().min(input.remaining());
        let mut count = accepted;
        while count > 0 {
            let chunk = input.chunk();
            let taken = chunk.len().min(count);
            self.append(&chunk[..taken]);
            input.advance(taken);
            count -= taken;
        }
        Poll::Ready(Ok(accepted))
    }
    fn poll_flush(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
pub struct SendConn<L> {
    streams: Vec<L>,
    states: Vec<Arc<SendState>>,
    waker: Arc<AtomicWaker>,
}

/// Take packets at the input's pace from the streams that have bytes, in
/// turn, until every stream is closed and empty, checking each stream's
/// frames after unlocking. Returns the frame checksum and the waits for the
/// stream locks.
async fn drain<L: Lock<Value = SendStream>>(
    conn: Arc<SendConn<L>>,
    pace: Pace,
    counters: Arc<Counters>,
) -> (u64, Vec<Duration>) {
    let streams = conn.streams.len();
    let mut received: Vec<BytesMut> = (0..streams).map(|_| BytesMut::new()).collect();
    let mut decoders: Vec<FrameDecoder<Http3>> = (0..streams).map(|_| Default::default()).collect();
    let mut processor = Processor::<Http3>::new(Work::CHECK);
    let mut pacer = Pacer::new(pace);
    let mut packet = Vec::with_capacity(PACKET);
    let mut waits = Vec::new();
    let mut next = 0;
    loop {
        let found = poll_fn(|cx| {
            poll_find(&conn.waker, cx, || {
                let mut open = false;
                for turn in 0..streams {
                    let index = (next + turn) % streams;
                    let state = &conn.states[index];
                    // Read `closed` first: every byte was buffered before it.
                    let closed = state.closed.load(Ordering::Acquire);
                    if state.buffered.load(Ordering::Acquire) > 0 {
                        return Poll::Ready(Some(index));
                    }
                    open |= !closed;
                }
                if open {
                    Poll::Pending
                } else {
                    Poll::Ready(None)
                }
            })
        })
        .await;
        let Some(index) = found else {
            assert!(received.iter().all(BytesMut::is_empty), "incomplete frame");
            return (processor.checksum, waits);
        };
        next = index + 1;
        let mut attempt = TimedLock::new(&conn.streams[index], &counters.other);
        let count = poll_fn(|cx| {
            let mut stream = ready!(attempt.poll_lock(cx, &mut waits));
            let count = PACKET.min(stream.buffer.len());
            packet.clear();
            packet.extend_from_slice(&stream.buffer[..count]);
            stream.buffer.advance(count);
            stream.moved += count;
            let buffered = stream.buffer.len();
            stream.state.buffered.store(buffered, Ordering::Release);
            if stream.credit() >= stream.wanted {
                wake(&mut stream.writer);
            }
            Poll::Ready(count)
        })
        .await;
        received[index].extend_from_slice(&packet);
        while let Some(item) = decoders[index].decode(&mut received[index]).unwrap() {
            processor.add(item);
        }
        pacer.admit(count).await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SendVariant {
    PlainLockPerLeaf,
    /// One call writes every frame, keeping the view until the end of each
    /// poll.
    Call,
    /// The same, releasing the view at every release point.
    CallRelease,
    /// The sans-IO way: encode frames ahead of the lock, then copy as many
    /// whole frames as fit into the stream buffer.
    CodecDirect,
    /// `FramedWrite` over an `AsyncWrite` of the stream: encode frames into
    /// its own buffer, written out whenever that reaches 8 KiB.
    CodecFramed,
}
pub const SENDS: [SendVariant; 5] = [
    SendVariant::PlainLockPerLeaf,
    SendVariant::Call,
    SendVariant::CallRelease,
    SendVariant::CodecDirect,
    SendVariant::CodecFramed,
];

/// Frames encoded ahead of the lock: their headers and field sections in
/// one buffer, and DATA payloads borrowed from the messages.
#[derive(Default)]
struct Staged<'m> {
    encoded: BytesMut,
    /// Each frame's encoded length and payload.
    frames: VecDeque<(usize, &'m [u8])>,
    size: usize,
}
impl<'m> Staged<'m> {
    fn push(&mut self, frame: Frame<'m>, fields: &mut Vec<u8>, header: &mut Vec<u8>, work: Work) {
        frame.prepare(work);
        let body = match frame {
            Frame::Data { payload, .. } => payload,
            Frame::Headers { .. } => &[],
        };
        let (header, payload) = frame.parts::<Http3>(fields, header);
        let mut encoded = header.len();
        self.encoded.extend_from_slice(header);
        if body.is_empty() {
            encoded += payload.len();
            self.encoded.extend_from_slice(payload);
        }
        assert!(
            encoded + body.len() <= WINDOW,
            "a whole frame must fit in the stream buffer"
        );
        self.frames.push_back((encoded, body));
        self.size += encoded + body.len();
    }
    fn front_size(&self) -> Option<usize> {
        self.frames
            .front()
            .map(|(encoded, body)| encoded + body.len())
    }
    fn pop_into(&mut self, stream: &mut SendStream) {
        let (encoded, body) = self.frames.pop_front().unwrap();
        stream.append(&self.encoded[..encoded]);
        self.encoded.advance(encoded);
        stream.append(body);
        self.size -= encoded + body.len();
    }
}

/// An `AsyncWrite` of one stream that locks on each write.
struct StreamWriter<'p, L: Lock> {
    attempt: PollLock<'p, L>,
}
impl<L: Lock<Value = SendStream>> AsyncWrite for StreamWriter<'_, L> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut stream = ready!(self.attempt.poll_lock(cx));
        ByteSink::poll_write(&mut *stream, cx, &mut &*input)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

async fn send<L: Lock<Value = SendStream>>(
    variant: SendVariant,
    stream: &L,
    index: usize,
    counters: &Counters,
    input: &Input,
) {
    let frames = slot_frames(&input.messages, index, input.streams, input.chunk);
    let (mut fields, mut header) = (Vec::<u8>::new(), Vec::<u8>::new());
    match variant {
        SendVariant::PlainLockPerLeaf => {
            let mut sink = LockPerLeaf::new(stream, &counters.locks);
            for frame in frames {
                frame.prepare(input.work);
                let (header, payload) = frame.parts::<Http3>(&mut fields, &mut header);
                plain_write_all(&mut sink, header).await.unwrap();
                plain_write_all(&mut sink, payload).await.unwrap();
            }
        }
        SendVariant::Call | SendVariant::CallRelease => {
            let release = match variant {
                SendVariant::CallRelease => Release::Always,
                _ => Release::Never,
            };
            let mut executor = Shared {
                lock: stream,
                stats: &counters.locks,
                release,
            };
            executor
                .write_slot_unpin(
                    Http3,
                    input.messages.clone(),
                    index,
                    input.streams,
                    input.chunk,
                    input.work,
                )
                .await
                .unwrap()
        }
        SendVariant::CodecDirect => {
            let mut frames = frames;
            let mut staged = Staged::default();
            // Whether a wakeup for room was asked for at the last unlock.
            let mut waiting = false;
            loop {
                while staged.size < WINDOW {
                    let Some(frame) = frames.next() else { break };
                    staged.push(frame, &mut fields, &mut header, input.work);
                }
                if staged.frames.is_empty() {
                    break;
                }
                if waiting {
                    woken().await;
                }
                let mut attempt = PollLock::new(stream, &counters.locks);
                waiting = poll_fn(|cx| {
                    let mut stream = ready!(attempt.poll_lock(cx));
                    let mut copied = false;
                    while let Some(size) = staged.front_size()
                        && size <= stream.credit()
                    {
                        staged.pop_into(&mut stream);
                        copied = true;
                    }
                    match staged.front_size() {
                        None => Poll::Ready(false),
                        Some(size) => {
                            stream.wait_for_room(cx, size);
                            if copied {
                                Poll::Ready(true)
                            } else {
                                Poll::Pending
                            }
                        }
                    }
                })
                .await;
            }
        }
        SendVariant::CodecFramed => {
            let writer = StreamWriter {
                attempt: PollLock::new(stream, &counters.locks),
            };
            let mut sink = FramedWrite::new(writer, FrameEncoder::<Http3>::default());
            for frame in frames {
                poll_fn(|cx| Pin::new(&mut sink).poll_ready(cx))
                    .await
                    .unwrap();
                frame.prepare(input.work);
                Pin::new(&mut sink).start_send(frame).unwrap();
                counters.buffer_at_least(sink.write_buffer().capacity());
            }
            poll_fn(|cx| Pin::new(&mut sink).poll_flush(cx))
                .await
                .unwrap();
        }
    }
}

struct RunSend<'r> {
    runtime: &'r Runtime,
    variant: SendVariant,
    input: Input,
    holds: bool,
}
impl PerLock<SendStream> for RunSend<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = SendStream>>(self) -> Sample {
        let RunSend {
            runtime,
            variant,
            input,
            holds,
        } = self;
        let expected = input.written;
        runtime.block_on(async move {
            let waker = Arc::new(AtomicWaker::new());
            let states: Vec<_> = (0..input.streams)
                .map(|_| Arc::new(SendState::default()))
                .collect();
            let streams = states
                .iter()
                .map(|state| {
                    L::new(SendStream {
                        buffer: BytesMut::with_capacity(WINDOW),
                        writer: None,
                        wanted: 0,
                        update: input.update,
                        closed: false,
                        moved: 0,
                        state: state.clone(),
                        connection: waker.clone(),
                    })
                })
                .collect();
            let conn = Arc::new(SendConn {
                streams,
                states,
                waker,
            });
            let counters = Arc::new(Counters::new(holds));
            let start = Instant::now();
            // Joined below; it ends once every stream is closed and empty.
            let drainer = tokio::spawn(drain(conn.clone(), input.pace, counters.clone()));
            per_stream(input.streams, |index| {
                let (conn, counters, input) = (conn.clone(), counters.clone(), input.clone());
                async move {
                    let stream = &conn.streams[index];
                    counted(
                        &counters.polls,
                        send(variant, stream, index, &counters, &input),
                    )
                    .await;
                    let mut attempt = PollLock::new(stream, &counters.locks);
                    poll_fn(|cx| {
                        ready!(attempt.poll_lock(cx)).close();
                        Poll::Ready(())
                    })
                    .await;
                    0
                }
            })
            .await;
            let (checksum, waits) = drainer.await.unwrap();
            let elapsed = start.elapsed();
            assert_eq!(checksum, expected, "{variant:?}");
            Sample::new(elapsed, &counters, waits)
        })
    }
}

pub fn run_send(
    runtime: &Runtime,
    variant: SendVariant,
    lock: LockKind,
    input: Input,
    holds: bool,
) -> Sample {
    with_lock(
        lock,
        RunSend {
            runtime,
            variant,
            input,
            holds,
        },
    )
}

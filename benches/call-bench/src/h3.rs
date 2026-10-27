//! HTTP/3 with QPACK over the stream buffers of a QUIC connection in user
//! space. Every stream's buffer lives in one connection state behind one
//! lock, and one task per stream reads or writes its stream.
//!
//! Receiving: a connection task delivers packets of up to [`PACKET`] bytes,
//! taking the streams in turn, while a stream holds less than [`WINDOW`]
//! bytes. Sending: each stream may hold [`WINDOW`] bytes; a connection task
//! takes up to [`PACKET`] bytes from the streams in turn, returns their
//! credit, and checks the frames after unlocking. Holding the lock while
//! waiting would keep the connection task from making progress, so no
//! variant does.
use crate::{
    calls::{
        ReadAllHostExt as _, ReadFrameHostExt as _, Shared, WriteFrameHostExt as _,
        WriteSlotHostExt as _,
    },
    codec::{FrameDecoder, FrameEncoder, Item, Processor},
    common::{
        ByteSink, ByteSource, Counters, Lock, LockKind, LockPerPoll, PerLock, PollLock, Project,
        Sample, TimedLock, bump, counted, plain_read_frame, plain_write_all, with_lock,
    },
    fields::Fields,
    workload::{Http3, Message, encode_all, slot_frames, total_checksum},
};
use bytes::{Buf, BytesMut};
use futures_sink::Sink;
use std::{
    future::poll_fn,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker, ready},
    time::{Duration, Instant},
};
use tokio::{io::AsyncWrite, runtime::Runtime};
use tokio_util::codec::{Decoder, FramedWrite};

/// The most bytes a stream buffer may hold.
pub const WINDOW: usize = 64 * 1024;
/// Stream bytes per packet.
pub const PACKET: usize = 1350;

#[derive(Clone, Copy)]
pub struct StreamIndex(pub usize);

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
    /// Yield to the scheduler after each packet.
    pub trickle: bool,
    /// Each stream's frames, encoded.
    pub bytes: Arc<Vec<Vec<u8>>>,
    pub checksum: u64,
}
impl Input {
    pub fn new(messages: Arc<Vec<Message>>, streams: usize, chunk: usize, trickle: bool) -> Self {
        let bytes = (0..streams)
            .map(|slot| encode_all::<Http3>(slot_frames(&messages, slot, streams, chunk)))
            .collect::<Vec<_>>()
            .into();
        let checksum = total_checksum(messages.iter());
        Self {
            messages,
            streams,
            chunk,
            trickle,
            bytes,
            checksum,
        }
    }
}

/// Spawn one task per stream and add up what they return.
async fn per_stream<F>(streams: usize, task: impl Fn(StreamIndex) -> F) -> u64
where
    F: Future<Output = u64> + Send + 'static,
{
    let tasks: Vec<_> = (0..streams)
        .map(|index| tokio::spawn(task(StreamIndex(index))))
        .collect();
    let mut total = 0;
    for task in tasks {
        total += task.await.unwrap();
    }
    total
}

// Receiving.

pub struct RecvStream {
    buffer: BytesMut,
    closed: bool,
    reader: Option<Waker>,
    producer: Option<Waker>,
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
    fn consumed(&mut self) {
        wake(&mut self.producer);
    }
}
pub struct RecvConn {
    streams: Vec<RecvStream>,
}
impl Project<RecvConn> for StreamIndex {
    type Target = RecvStream;
    fn project<'v>(&self, conn: &'v mut RecvConn) -> &'v mut RecvStream {
        &mut conn.streams[self.0]
    }
}

/// Deliver every stream's bytes, then close the streams. Returns the waits
/// for the lock.
async fn deliver<L: Lock<Value = RecvConn>>(shared: Arc<L>, input: Input) -> Vec<Duration> {
    let streams = input.streams;
    let mut offsets = vec![0; streams];
    let mut waits = Vec::new();
    let mut next = 0;
    loop {
        let mut attempt = TimedLock::new(&*shared);
        let finished = poll_fn(|cx| {
            let mut conn = ready!(attempt.poll_lock(cx, &mut waits));
            let mut open = false;
            for turn in 0..streams {
                let index = (next + turn) % streams;
                let data = &input.bytes[index];
                let stream = &mut conn.streams[index];
                if offsets[index] == data.len() {
                    if !stream.closed {
                        stream.closed = true;
                        wake(&mut stream.reader);
                    }
                    continue;
                }
                open = true;
                let space = WINDOW.saturating_sub(stream.buffer.len());
                if space == 0 {
                    stream.producer = Some(cx.waker().clone());
                    continue;
                }
                let count = PACKET.min(space).min(data.len() - offsets[index]);
                stream
                    .buffer
                    .extend_from_slice(&data[offsets[index]..offsets[index] + count]);
                offsets[index] += count;
                wake(&mut stream.reader);
                next = index + 1;
                return Poll::Ready(false);
            }
            if open {
                Poll::Pending
            } else {
                Poll::Ready(true)
            }
        })
        .await;
        if finished {
            return waits;
        }
        if input.trickle {
            tokio::task::yield_now().await;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecvVariant {
    PlainLockPerPoll,
    Call,
    CallAll,
    /// Lock on each poll and decode the next item from the stream buffer.
    Codec,
    /// Lock on each poll, decode every item that is ready and process them
    /// after unlocking.
    CodecBatch,
}
pub const RECVS: [RecvVariant; 5] = [
    RecvVariant::PlainLockPerPoll,
    RecvVariant::Call,
    RecvVariant::CallAll,
    RecvVariant::Codec,
    RecvVariant::CodecBatch,
];

async fn receive<L: Lock<Value = RecvConn>>(
    variant: RecvVariant,
    shared: &L,
    index: StreamIndex,
    counters: &Counters,
) -> u64 {
    let (mut block, mut fields) = (Vec::new(), Fields::default());
    let mut total = 0;
    let mut executor = Shared {
        lock: shared,
        project: index,
        locks: &counters.locks,
    };
    match variant {
        RecvVariant::PlainLockPerPoll => {
            let mut source = LockPerPoll::new(shared, index, &counters.locks);
            while let Some(checksum) =
                plain_read_frame::<Http3, _>(&mut source, &mut block, &mut fields)
                    .await
                    .unwrap()
            {
                total += checksum;
            }
            total
        }
        RecvVariant::Call => {
            while let Some(checksum) = executor
                .read_frame_unpin(Http3, &mut block, &mut fields)
                .await
                .unwrap()
            {
                total += checksum;
            }
            total
        }
        RecvVariant::CallAll => executor
            .read_all_unpin(Http3, &mut block, &mut fields)
            .await
            .unwrap(),
        RecvVariant::Codec | RecvVariant::CodecBatch => {
            let batch = variant == RecvVariant::CodecBatch;
            let mut codec = FrameDecoder::<Http3>::default();
            let mut processor = Processor::<Http3>::default();
            let mut items: Vec<Item> = Vec::new();
            let mut ended = false;
            while !ended {
                let mut attempt = PollLock::new(shared);
                poll_fn(|cx| {
                    let mut conn = ready!(attempt.poll_lock(cx));
                    bump(&counters.locks);
                    let stream = index.project(&mut *conn);
                    while let Some(item) = codec.decode(&mut stream.buffer).unwrap() {
                        items.push(item);
                        if !batch {
                            break;
                        }
                    }
                    if !items.is_empty() {
                        wake(&mut stream.producer);
                        Poll::Ready(())
                    } else if stream.closed {
                        assert!(stream.buffer.is_empty(), "incomplete frame");
                        ended = true;
                        Poll::Ready(())
                    } else {
                        stream.reader = Some(cx.waker().clone());
                        Poll::Pending
                    }
                })
                .await;
                for item in items.drain(..) {
                    processor.add(item);
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
}
impl PerLock<RecvConn> for RunRecv<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = RecvConn>>(self) -> Sample {
        let RunRecv {
            runtime,
            variant,
            input,
        } = self;
        let expected = input.checksum;
        runtime.block_on(async move {
            let streams = (0..input.streams)
                .map(|_| RecvStream {
                    buffer: BytesMut::with_capacity(WINDOW),
                    closed: false,
                    reader: None,
                    producer: None,
                })
                .collect();
            let shared = Arc::new(L::new(RecvConn { streams }));
            let counters = Arc::new(Counters::default());
            let start = Instant::now();
            // Joined below; it ends after closing every stream.
            let deliverer = tokio::spawn(deliver(shared.clone(), input.clone()));
            let checksum = per_stream(input.streams, |index| {
                let (shared, counters) = (shared.clone(), counters.clone());
                async move {
                    counted(
                        &counters.polls,
                        receive(variant, &*shared, index, &counters),
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

pub fn run_recv(runtime: &Runtime, variant: RecvVariant, lock: LockKind, input: Input) -> Sample {
    with_lock(
        lock,
        RunRecv {
            runtime,
            variant,
            input,
        },
    )
}

// Sending.

pub struct SendStream {
    buffer: BytesMut,
    writer: Option<Waker>,
    drainer: Option<Waker>,
    closed: bool,
}
impl SendStream {
    fn credit(&self) -> usize {
        WINDOW - self.buffer.len()
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
            self.writer = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let accepted = self.credit().min(input.remaining());
        let mut count = accepted;
        while count > 0 {
            let chunk = input.chunk();
            let taken = chunk.len().min(count);
            self.buffer.extend_from_slice(&chunk[..taken]);
            input.advance(taken);
            count -= taken;
        }
        wake(&mut self.drainer);
        Poll::Ready(Ok(accepted))
    }
    fn poll_flush(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
pub struct SendConn {
    streams: Vec<SendStream>,
}
impl Project<SendConn> for StreamIndex {
    type Target = SendStream;
    fn project<'v>(&self, conn: &'v mut SendConn) -> &'v mut SendStream {
        &mut conn.streams[self.0]
    }
}

/// Take packets from the streams in turn until every stream is closed and
/// empty, checking each stream's frames after unlocking. Returns the frame
/// checksum and the waits for the lock.
async fn drain<L: Lock<Value = SendConn>>(shared: Arc<L>, streams: usize) -> (u64, Vec<Duration>) {
    let mut received: Vec<BytesMut> = (0..streams).map(|_| BytesMut::new()).collect();
    let mut decoders: Vec<FrameDecoder<Http3>> = (0..streams).map(|_| Default::default()).collect();
    let mut processor = Processor::<Http3>::default();
    let mut packet = Vec::with_capacity(PACKET);
    let mut waits = Vec::new();
    let mut next = 0;
    loop {
        let mut attempt = TimedLock::new(&*shared);
        let taken = poll_fn(|cx| {
            let mut conn = ready!(attempt.poll_lock(cx, &mut waits));
            let mut open = false;
            for turn in 0..streams {
                let index = (next + turn) % streams;
                let stream = &mut conn.streams[index];
                if stream.buffer.is_empty() {
                    if !stream.closed {
                        open = true;
                        stream.drainer = Some(cx.waker().clone());
                    }
                    continue;
                }
                let count = PACKET.min(stream.buffer.len());
                packet.clear();
                packet.extend_from_slice(&stream.buffer[..count]);
                stream.buffer.advance(count);
                wake(&mut stream.writer);
                next = index + 1;
                return Poll::Ready(Some(index));
            }
            if open {
                Poll::Pending
            } else {
                Poll::Ready(None)
            }
        })
        .await;
        let Some(index) = taken else {
            assert!(received.iter().all(BytesMut::is_empty), "incomplete frame");
            return (processor.checksum, waits);
        };
        received[index].extend_from_slice(&packet);
        while let Some(item) = decoders[index].decode(&mut received[index]).unwrap() {
            processor.add(item);
        }
    }
}

async fn close<L: Lock<Value = SendConn>>(shared: &L, index: StreamIndex) {
    let mut conn = crate::common::lock(shared).await;
    let stream = index.project(&mut *conn);
    stream.closed = true;
    wake(&mut stream.drainer);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SendVariant {
    PlainLockPerPoll,
    Call,
    CallAll,
    /// The sans-IO way: encode the frame's field section, then copy the whole
    /// frame into the stream buffer under the lock once it has room.
    CodecDirect,
    /// `FramedWrite` over an `AsyncWrite` of the stream: encode frames into
    /// its own buffer, written out whenever that reaches 8 KiB.
    CodecFramed,
}
pub const SENDS: [SendVariant; 5] = [
    SendVariant::PlainLockPerPoll,
    SendVariant::Call,
    SendVariant::CallAll,
    SendVariant::CodecDirect,
    SendVariant::CodecFramed,
];

/// An `AsyncWrite` of one stream that locks on each write.
struct StreamWriter<'p, L: Lock> {
    attempt: PollLock<'p, L>,
    index: StreamIndex,
    counters: &'p Counters,
}
impl<L: Lock<Value = SendConn>> AsyncWrite for StreamWriter<'_, L> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        let mut conn = ready!(this.attempt.poll_lock(cx));
        bump(&this.counters.locks);
        this.index.project(&mut *conn).poll_write(cx, &mut &*input)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

async fn send<L: Lock<Value = SendConn>>(
    variant: SendVariant,
    shared: &L,
    index: StreamIndex,
    counters: &Counters,
    input: &Input,
) {
    let frames = slot_frames(&input.messages, index.0, input.streams, input.chunk);
    let (mut fields, mut header) = (Vec::<u8>::new(), Vec::<u8>::new());
    let mut executor = Shared {
        lock: shared,
        project: index,
        locks: &counters.locks,
    };
    match variant {
        SendVariant::PlainLockPerPoll => {
            let mut sink = LockPerPoll::new(shared, index, &counters.locks);
            for frame in frames {
                let (header, payload) = frame.parts::<Http3>(&mut fields, &mut header);
                plain_write_all(&mut sink, header).await.unwrap();
                plain_write_all(&mut sink, payload).await.unwrap();
            }
        }
        SendVariant::Call => {
            for frame in frames {
                executor
                    .write_frame_unpin(Http3, frame, &mut fields, &mut header)
                    .await
                    .unwrap();
            }
        }
        SendVariant::CallAll => executor
            .write_slot_unpin(
                Http3,
                input.messages.clone(),
                index.0,
                input.streams,
                input.chunk,
            )
            .await
            .unwrap(),
        SendVariant::CodecDirect => {
            for frame in frames {
                let (header, payload) = frame.parts::<Http3>(&mut fields, &mut header);
                let size = header.len() + payload.len();
                assert!(
                    size <= WINDOW,
                    "a whole frame must fit in the stream buffer"
                );
                let mut attempt = PollLock::new(shared);
                poll_fn(|cx| {
                    let mut conn = ready!(attempt.poll_lock(cx));
                    bump(&counters.locks);
                    let stream = index.project(&mut *conn);
                    if stream.credit() < size {
                        stream.writer = Some(cx.waker().clone());
                        return Poll::Pending;
                    }
                    stream.buffer.extend_from_slice(header);
                    stream.buffer.extend_from_slice(payload);
                    wake(&mut stream.drainer);
                    Poll::Ready(())
                })
                .await;
            }
        }
        SendVariant::CodecFramed => {
            let writer = StreamWriter {
                attempt: PollLock::new(shared),
                index,
                counters,
            };
            let mut sink = FramedWrite::new(writer, FrameEncoder::<Http3>::default());
            for frame in frames {
                poll_fn(|cx| Pin::new(&mut sink).poll_ready(cx))
                    .await
                    .unwrap();
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
}
impl PerLock<SendConn> for RunSend<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = SendConn>>(self) -> Sample {
        let RunSend {
            runtime,
            variant,
            input,
        } = self;
        let expected = input.checksum;
        runtime.block_on(async move {
            let streams = (0..input.streams)
                .map(|_| SendStream {
                    buffer: BytesMut::with_capacity(WINDOW),
                    writer: None,
                    drainer: None,
                    closed: false,
                })
                .collect();
            let shared = Arc::new(L::new(SendConn { streams }));
            let counters = Arc::new(Counters::default());
            let start = Instant::now();
            // Joined below; it ends once every stream is closed and empty.
            let drainer = tokio::spawn(drain(shared.clone(), input.streams));
            per_stream(input.streams, |index| {
                let (shared, counters, input) = (shared.clone(), counters.clone(), input.clone());
                async move {
                    counted(
                        &counters.polls,
                        send(variant, &*shared, index, &counters, &input),
                    )
                    .await;
                    close(&*shared, index).await;
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

pub fn run_send(runtime: &Runtime, variant: SendVariant, lock: LockKind, input: Input) -> Sample {
    with_lock(
        lock,
        RunSend {
            runtime,
            variant,
            input,
        },
    )
}

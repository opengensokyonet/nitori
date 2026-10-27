//! HTTP/2 with HPACK over a tokio duplex pipe, as over a TCP socket.
//!
//! Each read from or write to the pipe takes a lock inside tokio, as a system
//! call would cost a kernel transition, so every reader keeps an 8 KiB buffer
//! and every writer buffers up to 8 KiB before writing (larger writes go
//! straight to the pipe when the buffer is empty).
//!
//! Reads: a feeder task writes the connection's frames in 1460-byte segments;
//! the reader decodes every field section and visits every DATA payload.
//! Writes: the writer encodes field sections and writes frames; a checker
//! task reads them back.
use crate::{
    calls::{
        Consume, Flush, FlushHostExt as _, ReadAllHostExt as _, ReadFrameHostExt as _, Shared,
        Write, WriteFrameHostExt as _, WriteFramesHostExt as _,
    },
    codec::{FrameDecoder, FrameEncoder, Item, Processor},
    common::{
        ByteSink, ByteSource, Counters, Lock, LockKind, LockPerPoll, PerLock, PollLock, Sample,
        Whole, bump, counted, lock, plain_read_frame, plain_write_all, poll_read_into, probe,
        with_lock,
    },
    fields::Fields,
    workload::{Http2, Message, connection_frames, encode_all, total_checksum},
};
use bytes::{Buf, BytesMut};
use futures_core::Stream;
use futures_sink::Sink;
use nitori_call::{BorrowedReceiver, family_receiver};
use std::{
    future::poll_fn,
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, ready},
    time::Instant,
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt, DuplexStream},
    runtime::Runtime,
};
use tokio_util::codec::{FramedRead, FramedWrite};

const BUFFER: usize = 8 * 1024;

pub struct BufferedPipe {
    pipe: DuplexStream,
    buffer: BytesMut,
}
impl BufferedPipe {
    pub fn new(pipe: DuplexStream) -> Self {
        Self {
            pipe,
            buffer: BytesMut::with_capacity(BUFFER),
        }
    }
}
impl ByteSource for BufferedPipe {
    fn buffer(&mut self) -> &mut BytesMut {
        &mut self.buffer
    }
    fn poll_more(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<bool>> {
        // The buffer is empty, so this reuses its allocation.
        self.buffer.reserve(BUFFER);
        let count = ready!(poll_read_into(&mut self.pipe, cx, &mut self.buffer))?;
        Poll::Ready(Ok(count > 0))
    }
}
family_receiver!(impl [] for BufferedPipe);
impl Consume for BufferedPipe {
    fn poll_consume<'visit, F: FnMut(&[u8]) + ?Sized>(
        host: Pin<&mut BorrowedReceiver<'visit, BufferedPipe>>,
        cx: &mut Context<'_>,
        maximum: usize,
        visit: &mut F,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit,
    {
        ByteSource::poll_consume(host.get_mut().0.as_mut().get_mut(), cx, maximum, visit)
    }
}

pub struct PipeWriter {
    pipe: DuplexStream,
    buffer: BytesMut,
}
impl PipeWriter {
    fn new(pipe: DuplexStream) -> Self {
        Self {
            pipe,
            buffer: BytesMut::with_capacity(BUFFER),
        }
    }
}
impl ByteSink for PipeWriter {
    fn poll_write<I: Buf + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>> {
        if self.buffer.is_empty() && input.remaining() >= BUFFER {
            let count = ready!(Pin::new(&mut self.pipe).poll_write(cx, input.chunk()))?;
            input.advance(count);
            return Poll::Ready(Ok(count));
        }
        if self.buffer.len() >= BUFFER {
            ready!(self.poll_flush(cx))?;
        }
        let count = (BUFFER - self.buffer.len()).min(input.remaining());
        let mut left = count;
        while left > 0 {
            let chunk = input.chunk();
            let taken = chunk.len().min(left);
            self.buffer.extend_from_slice(&chunk[..taken]);
            input.advance(taken);
            left -= taken;
        }
        Poll::Ready(Ok(count))
    }
    fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while !self.buffer.is_empty() {
            let count = ready!(Pin::new(&mut self.pipe).poll_write(cx, &self.buffer))?;
            self.buffer.advance(count);
        }
        Poll::Ready(Ok(()))
    }
}
family_receiver!(impl [] for PipeWriter);
impl Write for PipeWriter {
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut BorrowedReceiver<'visit, PipeWriter>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit,
    {
        ByteSink::poll_write(host.get_mut().0.as_mut().get_mut(), cx, input)
    }
}
impl Flush for PipeWriter {
    fn poll_flush<'visit>(
        host: Pin<&mut BorrowedReceiver<'visit, PipeWriter>>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>>
    where
        Self: 'visit,
    {
        ByteSink::poll_flush(host.get_mut().0.as_mut().get_mut(), cx)
    }
}

#[derive(Clone)]
pub struct Input {
    pub messages: Arc<Vec<Message>>,
    pub streams: usize,
    pub chunk: usize,
    /// Yield to the scheduler after each segment, so the reader waits for
    /// nearly every segment.
    pub trickle: bool,
    /// The connection's frames, encoded.
    pub bytes: Arc<Vec<u8>>,
    pub checksum: u64,
}
impl Input {
    pub fn new(messages: Arc<Vec<Message>>, streams: usize, chunk: usize, trickle: bool) -> Self {
        assert!(
            chunk <= 16 * 1024,
            "DATA frames are limited to the default SETTINGS_MAX_FRAME_SIZE"
        );
        let bytes = encode_all::<Http2>(connection_frames(&messages, streams, chunk)).into();
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

async fn feed(input: Input, mut client: DuplexStream) {
    for segment in input.bytes.chunks(1460) {
        client.write_all(segment).await.unwrap();
        if input.trickle {
            tokio::task::yield_now().await;
        }
    }
}

/// Read every frame with ordinary async.
async fn plain_read_all<S: crate::common::PollConsume + ?Sized>(source: &mut S) -> u64 {
    let (mut block, mut fields) = (Vec::new(), Fields::default());
    let mut total = 0;
    while let Some(checksum) = plain_read_frame::<Http2, _>(source, &mut block, &mut fields)
        .await
        .unwrap()
    {
        total += checksum;
    }
    total
}

// Reads.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Read {
    Plain,
    Call,
    CallAll,
    Codec,
}
pub const READS: [Read; 4] = [Read::Plain, Read::Call, Read::CallAll, Read::Codec];

pub fn run_read(runtime: &Runtime, variant: Read, input: Input) -> Sample {
    let expected = input.checksum;
    runtime.block_on(async move {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let start = Instant::now();
        // Joined below; it ends after writing every frame.
        let feeder = tokio::spawn(feed(input, client));
        let counters = Counters::default();
        let mut pipe = BufferedPipe::new(server);
        let read = async {
            let (mut block, mut fields) = (Vec::new(), Fields::default());
            match variant {
                Read::Plain => plain_read_all(&mut pipe).await,
                Read::Call => {
                    let mut total = 0;
                    while let Some(checksum) = pipe
                        .read_frame_unpin(Http2, &mut block, &mut fields)
                        .await
                        .unwrap()
                    {
                        total += checksum;
                    }
                    total
                }
                Read::CallAll => pipe
                    .read_all_unpin(Http2, &mut block, &mut fields)
                    .await
                    .unwrap(),
                Read::Codec => {
                    let mut framed = FramedRead::new(pipe.pipe, FrameDecoder::<Http2>::default());
                    let mut processor = Processor::<Http2>::default();
                    while let Some(item) = poll_fn(|cx| Pin::new(&mut framed).poll_next(cx)).await {
                        processor.add(item.unwrap());
                        processor.note_buffer(&counters, framed.read_buffer());
                    }
                    processor.checksum
                }
            }
        };
        let checksum = counted(&counters.polls, read).await;
        let elapsed = start.elapsed();
        feeder.await.unwrap();
        assert_eq!(checksum, expected, "{variant:?}");
        Sample::new(elapsed, &counters, Vec::new())
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SharedRead {
    /// Lock for each frame, holding the lock while waiting for its bytes.
    PlainHold,
    PlainLockPerPoll,
    Call,
    CallAll,
    /// Lock on each poll for the next item.
    Codec,
    /// Lock on each poll and decode every item that is ready, then process
    /// them after unlocking.
    CodecBatch,
}
pub const SHARED_READS: [SharedRead; 6] = [
    SharedRead::PlainHold,
    SharedRead::PlainLockPerPoll,
    SharedRead::Call,
    SharedRead::CallAll,
    SharedRead::Codec,
    SharedRead::CodecBatch,
];
impl SharedRead {
    pub fn supports(self, lock: LockKind) -> bool {
        // A blocking lock's guard must not be held across an await.
        !(self == Self::PlainHold && lock == LockKind::Blocking)
    }
}

struct RunShared<'r> {
    runtime: &'r Runtime,
    variant: SharedRead,
    input: Input,
    contention: bool,
}
impl RunShared<'_> {
    /// Run `read` on a shared value made from the pipe, with the feeder and
    /// optionally a probe running.
    fn run_with<L: Lock>(
        self,
        make: impl FnOnce(DuplexStream) -> L::Value,
        read: impl AsyncFnOnce(&L, &Counters) -> u64,
    ) -> Sample {
        let RunShared {
            runtime,
            variant,
            input,
            contention,
        } = self;
        let expected = input.checksum;
        runtime.block_on(async move {
            let (client, server) = tokio::io::duplex(64 * 1024);
            let start = Instant::now();
            // Joined below; it ends after writing every frame.
            let feeder = tokio::spawn(feed(input, client));
            let shared = Arc::new(L::new(make(server)));
            let stop = Arc::new(AtomicBool::new(false));
            // Joined below; it ends once `stop` is set.
            let prober = contention.then(|| tokio::spawn(probe(shared.clone(), stop.clone())));
            let counters = Counters::default();
            let checksum = counted(&counters.polls, read(&shared, &counters)).await;
            let elapsed = start.elapsed();
            stop.store(true, Ordering::Relaxed);
            let waits = match prober {
                Some(prober) => prober.await.unwrap(),
                None => Vec::new(),
            };
            feeder.await.unwrap();
            assert_eq!(checksum, expected, "{variant:?}");
            Sample::new(elapsed, &counters, waits)
        })
    }
}

impl PerLock<BufferedPipe> for RunShared<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = BufferedPipe>>(self) -> Sample {
        let variant = self.variant;
        self.run_with::<L>(BufferedPipe::new, async move |shared, counters| {
            let (mut block, mut fields) = (Vec::new(), Fields::default());
            let mut executor = Shared {
                lock: shared,
                project: Whole,
                locks: &counters.locks,
            };
            match variant {
                SharedRead::PlainHold => {
                    let mut total = 0;
                    loop {
                        let mut pipe = lock(shared).await;
                        bump(&counters.locks);
                        match plain_read_frame::<Http2, _>(&mut *pipe, &mut block, &mut fields)
                            .await
                            .unwrap()
                        {
                            Some(checksum) => total += checksum,
                            None => break total,
                        }
                    }
                }
                SharedRead::PlainLockPerPoll => {
                    plain_read_all(&mut LockPerPoll::new(shared, Whole, &counters.locks)).await
                }
                SharedRead::Call => {
                    let mut total = 0;
                    while let Some(checksum) = executor
                        .read_frame_unpin(Http2, &mut block, &mut fields)
                        .await
                        .unwrap()
                    {
                        total += checksum;
                    }
                    total
                }
                SharedRead::CallAll => executor
                    .read_all_unpin(Http2, &mut block, &mut fields)
                    .await
                    .unwrap(),
                SharedRead::Codec | SharedRead::CodecBatch => unreachable!(),
            }
        })
    }
}

type Framed = FramedRead<DuplexStream, FrameDecoder<Http2>>;
impl PerLock<Framed> for RunShared<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = Framed>>(self) -> Sample {
        let batch = self.variant == SharedRead::CodecBatch;
        self.run_with::<L>(
            |pipe| FramedRead::new(pipe, FrameDecoder::default()),
            async move |shared, counters| {
                let mut processor = Processor::<Http2>::default();
                let mut items: Vec<Item> = Vec::new();
                let mut ended = false;
                while !ended {
                    let mut attempt = PollLock::new(shared);
                    poll_fn(|cx| {
                        let mut framed = ready!(attempt.poll_lock(cx));
                        bump(&counters.locks);
                        loop {
                            match Pin::new(&mut *framed).poll_next(cx) {
                                Poll::Ready(Some(item)) => {
                                    items.push(item.unwrap());
                                    if !batch {
                                        break;
                                    }
                                }
                                Poll::Ready(None) => {
                                    ended = true;
                                    break;
                                }
                                Poll::Pending => break,
                            }
                        }
                        counters.buffer_at_least(framed.read_buffer().capacity());
                        if items.is_empty() && !ended {
                            Poll::Pending
                        } else {
                            Poll::Ready(())
                        }
                    })
                    .await;
                    for item in items.drain(..) {
                        processor.add(item);
                    }
                }
                processor.checksum
            },
        )
    }
}

pub fn run_shared_read(
    runtime: &Runtime,
    variant: SharedRead,
    lock: LockKind,
    input: Input,
    contention: bool,
) -> Sample {
    let run = RunShared {
        runtime,
        variant,
        input,
        contention,
    };
    match variant {
        SharedRead::Codec | SharedRead::CodecBatch => with_lock::<Framed, _>(lock, run),
        _ => with_lock::<BufferedPipe, _>(lock, run),
    }
}

// Writes.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WriteVariant {
    Plain,
    Call,
    CallAll,
    /// `FramedWrite`: encode frames into its own buffer, which it writes out
    /// whenever it reaches 8 KiB.
    Codec,
}
pub const WRITES: [WriteVariant; 4] = [
    WriteVariant::Plain,
    WriteVariant::Call,
    WriteVariant::CallAll,
    WriteVariant::Codec,
];

pub fn run_write(runtime: &Runtime, variant: WriteVariant, input: Input) -> Sample {
    let expected = input.checksum;
    runtime.block_on(async move {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let start = Instant::now();
        // Joined below; it ends at the end of the pipe.
        let checker =
            tokio::spawn(async move { plain_read_all(&mut BufferedPipe::new(client)).await });
        let counters = Counters::default();
        let frames = connection_frames(&input.messages, input.streams, input.chunk);
        let write = async {
            let mut writer = PipeWriter::new(server);
            match variant {
                WriteVariant::Plain => {
                    let (mut fields, mut header) = (Vec::new(), Vec::new());
                    for frame in frames {
                        let (header, payload) = frame.parts::<Http2>(&mut fields, &mut header);
                        plain_write_all(&mut writer, header).await.unwrap();
                        plain_write_all(&mut writer, payload).await.unwrap();
                    }
                    poll_fn(|cx| ByteSink::poll_flush(&mut writer, cx))
                        .await
                        .unwrap();
                }
                WriteVariant::Call => {
                    let (mut fields, mut header) = (Vec::new(), Vec::new());
                    for frame in frames {
                        writer
                            .write_frame_unpin(Http2, frame, &mut fields, &mut header)
                            .await
                            .unwrap();
                    }
                    writer.flush_unpin().await.unwrap();
                }
                WriteVariant::CallAll => {
                    writer.write_frames_unpin(Http2, frames).await.unwrap();
                    writer.flush_unpin().await.unwrap();
                }
                WriteVariant::Codec => {
                    let mut sink = FramedWrite::new(writer.pipe, FrameEncoder::<Http2>::default());
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
        };
        counted(&counters.polls, write).await;
        let checksum = checker.await.unwrap();
        let elapsed = start.elapsed();
        assert_eq!(checksum, expected, "{variant:?}");
        Sample::new(elapsed, &counters, Vec::new())
    })
}

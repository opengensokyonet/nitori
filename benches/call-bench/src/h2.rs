//! HTTP/2 with HPACK over a tokio duplex pipe, as over a TCP socket.
//!
//! Each read from or write to the pipe takes a lock inside tokio, as a system
//! call would cost a kernel transition, so every reader keeps an 8 KiB buffer
//! and every writer buffers up to 8 KiB before writing (larger writes go
//! straight to the pipe when the buffer is empty).
//!
//! Reads: a feeder task writes the connection's frames in 1460-byte segments
//! at the input's pace; the reader decodes every field section and visits
//! every DATA payload. With contention, a control task locks the shared
//! reader once for every so many bytes the feeder writes, as the frames a
//! connection answers (WINDOW_UPDATE, PING) would make it.
//! Writes: the writer encodes field sections and writes frames; a checker
//! task reads them back.
use crate::{
    calls::{
        Consume, Flush, FlushHostExt as _, ReadAllHostExt as _, Release, Shared, Take, Write,
        WriteFramesHostExt as _,
    },
    codec::{FrameDecoder, FrameEncoder, Item, Processor},
    common::{
        ByteSink, ByteSource, Counters, Lock, LockKind, LockPerLeaf, Pace, Pacer, PerLock,
        PollLock, Progress, Sample, TimedLock, counted, lock, plain_read_frame, plain_write_all,
        poll_read_into, with_lock, woken,
    },
    fields::Fields,
    workload::{Http2, Message, Work, connection_frames, encode_all, total_checksum},
};
use bytes::{Buf, BytesMut};
use futures_core::Stream;
use futures_sink::Sink;
use nitori_call::{BorrowedReceiver, family_receiver};
use std::{
    future::poll_fn,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt, DuplexStream},
    runtime::Runtime,
    sync::mpsc,
};
use tokio_util::codec::{FramedRead, FramedWrite};

const BUFFER: usize = 8 * 1024;

pub struct BufferedPipe {
    pipe: DuplexStream,
    buffer: BytesMut,
    /// Reads from the pipe and from the buffer.
    moved: u64,
}
impl BufferedPipe {
    pub fn new(pipe: DuplexStream) -> Self {
        Self {
            pipe,
            buffer: BytesMut::with_capacity(BUFFER),
            moved: 0,
        }
    }
}
impl Progress for BufferedPipe {
    fn progress(&self) -> u64 {
        self.moved
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
        self.moved += 1;
        Poll::Ready(Ok(count > 0))
    }
    fn consumed(&mut self, _: usize) {
        self.moved += 1;
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
        ByteSource::poll_consume(host.get_pin_mut().get_mut(), cx, maximum, visit)
    }
}
impl Take for BufferedPipe {
    fn poll_take<'visit>(
        host: Pin<&mut BorrowedReceiver<'visit, BufferedPipe>>,
        cx: &mut Context<'_>,
        maximum: usize,
    ) -> Poll<io::Result<BytesMut>>
    where
        Self: 'visit,
    {
        ByteSource::poll_take(host.get_mut().0.as_mut().get_mut(), cx, maximum)
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
        ByteSink::poll_write(host.get_pin_mut().get_mut(), cx, input)
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
        ByteSink::poll_flush(host.get_pin_mut().get_mut(), cx)
    }
}

#[derive(Clone)]
pub struct Input {
    pub messages: Arc<Vec<Message>>,
    pub streams: usize,
    pub chunk: usize,
    pub pace: Pace,
    /// Yield to the scheduler after each segment, so the reader waits for
    /// nearly every segment.
    pub trickle: bool,
    pub work: Work,
    /// The connection's frames, encoded.
    pub bytes: Arc<Vec<u8>>,
    /// What a reader doing `work` computes.
    pub checksum: u64,
    /// What the checker of written frames computes.
    pub written: u64,
}
impl Input {
    pub fn new(
        messages: Arc<Vec<Message>>,
        streams: usize,
        chunk: usize,
        pace: Pace,
        trickle: bool,
        work: Work,
    ) -> Self {
        assert!(
            chunk <= 16 * 1024,
            "DATA frames are limited to the default SETTINGS_MAX_FRAME_SIZE"
        );
        let bytes = encode_all::<Http2>(connection_frames(&messages, streams, chunk)).into();
        let checksum = total_checksum(messages.iter(), work);
        let written = total_checksum(messages.iter(), Work::CHECK);
        Self {
            messages,
            streams,
            chunk,
            pace,
            trickle,
            work,
            bytes,
            checksum,
            written,
        }
    }
}

/// Write the connection's bytes at the input's pace, telling `control` each
/// time another `every` bytes are written.
async fn feed(
    input: Input,
    mut client: DuplexStream,
    control: Option<(mpsc::UnboundedSender<()>, usize)>,
) {
    let mut pacer = Pacer::new(input.pace);
    let mut written = 0;
    for segment in input.bytes.chunks(1460) {
        pacer.admit(segment.len()).await;
        client.write_all(segment).await.unwrap();
        if let Some((ticks, every)) = &control
            && (written + segment.len()) / every > written / every
        {
            ticks.send(()).unwrap();
        }
        written += segment.len();
        if input.trickle {
            tokio::task::yield_now().await;
        }
    }
}

/// Lock the shared reader once for each tick from the feeder, recording how
/// long each acquisition waited. Ends with the feeder.
async fn control<L: Lock>(
    shared: Arc<L>,
    mut ticks: mpsc::UnboundedReceiver<()>,
    counters: Arc<Counters>,
) -> Vec<Duration> {
    let mut waits = Vec::new();
    while ticks.recv().await.is_some() {
        let mut attempt = TimedLock::new(&*shared, &counters.other);
        drop(poll_fn(|cx| attempt.poll_lock(cx, &mut waits)).await);
    }
    waits
}

/// Read every frame with ordinary async.
async fn plain_read_all<S: crate::common::PollConsume + ?Sized>(source: &mut S, work: Work) -> u64 {
    let (mut block, mut fields) = (Vec::new(), Fields::default());
    let mut total = 0;
    while let Some(checksum) = plain_read_frame::<Http2, _>(source, &mut block, &mut fields, work)
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
    /// One call reads every frame.
    Call,
    Codec,
}
pub const READS: [Read; 3] = [Read::Plain, Read::Call, Read::Codec];

pub fn run_read(runtime: &Runtime, variant: Read, input: Input) -> Sample {
    let (expected, work) = (input.checksum, input.work);
    runtime.block_on(async move {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let start = Instant::now();
        // Joined below; it ends after writing every frame.
        let feeder = tokio::spawn(feed(input, client, None));
        let counters = Counters::new(false);
        let mut pipe = BufferedPipe::new(server);
        let read = async {
            let (mut block, mut fields) = (Vec::new(), Fields::default());
            match variant {
                Read::Plain => plain_read_all(&mut pipe, work).await,
                Read::Call => pipe
                    .read_all_unpin(Http2, &mut block, &mut fields, work)
                    .await
                    .unwrap(),
                Read::Codec => {
                    let mut framed = FramedRead::new(pipe.pipe, FrameDecoder::<Http2>::default());
                    let mut processor = Processor::<Http2>::new(work);
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
    PlainLockPerLeaf,
    /// One call reads every frame, keeping the view until the end of each
    /// poll.
    Call,
    /// The same, releasing the view at every release point.
    CallRelease,
    /// Lock, decode every item that is ready, then process them after
    /// unlocking.
    Codec,
}
pub const SHARED_READS: [SharedRead; 5] = [
    SharedRead::PlainHold,
    SharedRead::PlainLockPerLeaf,
    SharedRead::Call,
    SharedRead::CallRelease,
    SharedRead::Codec,
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
    /// Bytes fed between the control task's locks, if it runs.
    contention: Option<usize>,
    holds: bool,
}
impl RunShared<'_> {
    /// Run `read` on a shared value made from the pipe, with the feeder and
    /// optionally the control task running.
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
            holds,
        } = self;
        let expected = input.checksum;
        runtime.block_on(async move {
            let (client, server) = tokio::io::duplex(64 * 1024);
            let shared = Arc::new(L::new(make(server)));
            let counters = Arc::new(Counters::new(holds));
            let (ticks, controller) = match contention {
                Some(every) => {
                    let (sender, receiver) = mpsc::unbounded_channel();
                    // Joined below; it ends after the feeder drops `sender`.
                    let controller =
                        tokio::spawn(control(shared.clone(), receiver, counters.clone()));
                    (Some((sender, every)), Some(controller))
                }
                None => (None, None),
            };
            let start = Instant::now();
            // Joined below; it ends after writing every frame.
            let feeder = tokio::spawn(feed(input, client, ticks));
            let checksum = counted(&counters.polls, read(&shared, &counters)).await;
            let elapsed = start.elapsed();
            feeder.await.unwrap();
            let waits = match controller {
                Some(controller) => controller.await.unwrap(),
                None => Vec::new(),
            };
            assert_eq!(checksum, expected, "{variant:?}");
            Sample::new(elapsed, &counters, waits)
        })
    }
}

impl PerLock<BufferedPipe> for RunShared<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = BufferedPipe>>(self) -> Sample {
        let (variant, work) = (self.variant, self.input.work);
        self.run_with::<L>(BufferedPipe::new, async move |shared, counters| {
            let (mut block, mut fields) = (Vec::new(), Fields::default());
            let release = match variant {
                SharedRead::CallRelease => Release::Always,
                _ => Release::Never,
            };
            let mut executor = Shared {
                lock: shared,
                stats: &counters.locks,
                release,
            };
            match variant {
                SharedRead::PlainHold => {
                    let mut total = 0;
                    loop {
                        let mut pipe = lock(shared, &counters.locks).await;
                        match plain_read_frame::<Http2, _>(
                            &mut *pipe,
                            &mut block,
                            &mut fields,
                            work,
                        )
                        .await
                        .unwrap()
                        {
                            Some(checksum) => total += checksum,
                            None => break total,
                        }
                    }
                }
                SharedRead::PlainLockPerLeaf => {
                    plain_read_all(&mut LockPerLeaf::new(shared, &counters.locks), work).await
                }
                SharedRead::Call | SharedRead::CallRelease => executor
                    .read_all_unpin(Http2, &mut block, &mut fields, work)
                    .await
                    .unwrap(),
                SharedRead::Codec => unreachable!(),
            }
        })
    }
}

type Framed = FramedRead<DuplexStream, FrameDecoder<Http2>>;
impl PerLock<Framed> for RunShared<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = Framed>>(self) -> Sample {
        let work = self.input.work;
        self.run_with::<L>(
            |pipe| FramedRead::new(pipe, FrameDecoder::default()),
            async move |shared, counters| {
                let mut processor = Processor::<Http2>::new(work);
                let mut items: Vec<Item> = Vec::new();
                let mut ended = false;
                while !ended {
                    // The reader asked for a wakeup before unlocking.
                    let mut waiting = false;
                    let mut attempt = PollLock::new(shared, &counters.locks);
                    poll_fn(|cx| {
                        let mut framed = ready!(attempt.poll_lock(cx));
                        loop {
                            match Pin::new(&mut *framed).poll_next(cx) {
                                Poll::Ready(Some(item)) => items.push(item.unwrap()),
                                Poll::Ready(None) => {
                                    ended = true;
                                    break;
                                }
                                Poll::Pending => {
                                    waiting = true;
                                    break;
                                }
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
                    if waiting {
                        woken().await;
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
    contention: Option<usize>,
    holds: bool,
) -> Sample {
    let run = RunShared {
        runtime,
        variant,
        input,
        contention,
        holds,
    };
    match variant {
        SharedRead::Codec => with_lock::<Framed, _>(lock, run),
        _ => with_lock::<BufferedPipe, _>(lock, run),
    }
}

// Writes.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WriteVariant {
    Plain,
    /// One call writes every frame.
    Call,
    /// `FramedWrite`: encode frames into its own buffer, which it writes out
    /// whenever it reaches 8 KiB.
    Codec,
}
pub const WRITES: [WriteVariant; 3] =
    [WriteVariant::Plain, WriteVariant::Call, WriteVariant::Codec];

pub fn run_write(runtime: &Runtime, variant: WriteVariant, input: Input) -> Sample {
    let (expected, work) = (input.written, input.work);
    runtime.block_on(async move {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let start = Instant::now();
        // Joined below; it ends at the end of the pipe.
        let checker = tokio::spawn(async move {
            plain_read_all(&mut BufferedPipe::new(client), Work::CHECK).await
        });
        let counters = Counters::new(false);
        let frames = connection_frames(&input.messages, input.streams, input.chunk);
        let write = async {
            let mut writer = PipeWriter::new(server);
            match variant {
                WriteVariant::Plain => {
                    let (mut fields, mut header) = (Vec::new(), Vec::new());
                    for frame in frames {
                        frame.prepare(work);
                        let (header, payload) = frame.parts::<Http2>(&mut fields, &mut header);
                        plain_write_all(&mut writer, header).await.unwrap();
                        plain_write_all(&mut writer, payload).await.unwrap();
                    }
                    poll_fn(|cx| ByteSink::poll_flush(&mut writer, cx))
                        .await
                        .unwrap();
                }
                WriteVariant::Call => {
                    writer
                        .write_frames_unpin(Http2, frames, work)
                        .await
                        .unwrap();
                    writer.flush_unpin().await.unwrap();
                }
                WriteVariant::Codec => {
                    let mut sink = FramedWrite::new(writer.pipe, FrameEncoder::<Http2>::default());
                    for frame in frames {
                        poll_fn(|cx| Pin::new(&mut sink).poll_ready(cx))
                            .await
                            .unwrap();
                        frame.prepare(work);
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

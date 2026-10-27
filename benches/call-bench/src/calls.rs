//! `nitori_call` operations over any byte source or sink, and the executor
//! that acquires a lock on the polls whose operations need the resource.
//!
//! The operations mark a release point before each computation that does not
//! need the resource: decoding a field section, visiting a piece of DATA
//! taken out of the source, and encoding a field section. Whether the
//! executor releases the view there is its [`Release`] policy.
use crate::{
    common::{ByteSink, ByteSource, Held, Lock, LockStats, PollLock},
    fields::Fields,
    workload::{Frame, FrameHeader, Kind, Message, Protocol, Work, slot_frames},
};
use bytes::{Buf, BytesMut};
use futures_core::Stream;
use nitori_call::{
    CallExecutor, Lend, Operation, ReceiverAccess, ReceiverFamily, ReceiverSlot, Start,
    ViewAcquisition, call, capability, drive,
};
use std::{
    future::Future,
    io,
    marker::PhantomData,
    ops::ControlFlow,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
};

/// Visit up to `maximum` readable bytes in place and consume them; `Ok(0)`
/// means the end of the source.
#[capability]
pub trait Consume: ReceiverFamily {
    fn poll_consume<'visit, F: FnMut(&[u8]) + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: usize,
        visit: &mut F,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit;
}

/// Take up to `maximum` readable bytes out of the source without copying; an
/// empty result means the end of the source.
#[capability]
pub trait Take: ReceiverFamily {
    fn poll_take<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: usize,
    ) -> Poll<io::Result<BytesMut>>
    where
        Self: 'visit;
}

/// Accept a prefix of `input`, or wait for room.
#[capability]
pub trait Write: ReceiverFamily {
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit;
}

/// Push out what was written.
#[capability]
pub trait Flush: ReceiverFamily {
    fn poll_flush<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>>
    where
        Self: 'visit;
}

#[call]
async fn consume_exact<F: FnMut(&[u8])>(
    io: &mut impl ReceiverAccess<Family: Consume>,
    length: usize,
    visit: &mut F,
) -> io::Result<()> {
    let mut remaining = length;
    while remaining > 0 {
        let count = io.consume(remaining, &mut *visit).await?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        remaining -= count;
    }
    Ok(())
}

/// Read one frame and return its checksum, or `None` at the end of the
/// source. A field section is copied into `block` and decoded into `fields`;
/// DATA is taken out of the source and processed with `work` after a release
/// point.
#[call]
async fn read_frame<P: Protocol>(
    io: &mut impl ReceiverAccess<Family: Consume + Take>,
    _protocol: P,
    block: &mut Vec<u8>,
    fields: &mut Fields,
    work: Work,
) -> io::Result<Option<u64>> {
    let mut header = [0u8; 16];
    let mut filled = 0;
    loop {
        let needed = P::header_needed(&header[..filled]);
        if needed == 0 {
            break;
        }
        let count = io
            .consume(needed, &mut |bytes: &[u8]| {
                header[filled..filled + bytes.len()].copy_from_slice(bytes)
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
            consume_exact(io, length, &mut |bytes: &[u8]| {
                block.extend_from_slice(bytes)
            })
            .await?;
            io.release_view().await;
            P::decode_fields(block, fields)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
            fields.checksum()
        }
        Kind::Data => {
            let mut checksum = 0;
            let mut remaining = length;
            while remaining > 0 {
                let piece = io.take(remaining).await?;
                if piece.is_empty() {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                remaining -= piece.len();
                io.release_view().await;
                checksum += work.read(&piece);
            }
            checksum
        }
    }))
}

/// Read every frame and return the total checksum.
#[call]
pub async fn read_all<P: Protocol>(
    io: &mut impl ReceiverAccess<Family: Consume + Take>,
    protocol: P,
    block: &mut Vec<u8>,
    fields: &mut Fields,
    work: Work,
) -> io::Result<u64> {
    let mut total = 0;
    while let Some(checksum) = read_frame(io, protocol, block, fields, work).await? {
        total += checksum;
    }
    Ok(total)
}

#[call]
async fn write_all(io: &mut impl ReceiverAccess<Family: Write>, input: &[u8]) -> io::Result<()> {
    let mut input = input;
    while input.has_remaining() {
        io.write(&mut input).await?;
    }
    Ok(())
}

/// Write one frame: its header, then its payload straight from the message,
/// processed with `work` first.
#[call]
async fn write_frame<P: Protocol>(
    io: &mut impl ReceiverAccess<Family: Write>,
    _protocol: P,
    frame: Frame<'_>,
    fields: &mut Vec<u8>,
    header: &mut Vec<u8>,
    work: Work,
) -> io::Result<()> {
    match frame {
        Frame::Headers { .. } => io.release_view().await,
        Frame::Data { payload, .. } if work.passes > 0 => {
            io.release_view().await;
            work.write(payload);
        }
        Frame::Data { .. } => {}
    }
    let (header, payload) = frame.parts::<P>(fields, header);
    write_all(io, header).await?;
    write_all(io, payload).await
}

/// Write every frame of `frames`.
#[call]
pub async fn write_frames<'m, P: Protocol, I: Iterator<Item = Frame<'m>>>(
    io: &mut impl ReceiverAccess<Family: Write>,
    protocol: P,
    frames: I,
    work: Work,
) -> io::Result<()> {
    let (mut fields, mut header) = (Vec::new(), Vec::new());
    for frame in frames {
        write_frame(io, protocol, frame, &mut fields, &mut header, work).await?;
    }
    Ok(())
}

/// Write the frames of stream slot `slot` of `streams` (see
/// [`slot_frames`]). It owns the messages, so the execution borrows nothing
/// but the executor and can run as its own task.
#[call]
pub async fn write_slot<P: Protocol>(
    io: &mut impl ReceiverAccess<Family: Write>,
    protocol: P,
    messages: Arc<Vec<Message>>,
    slot: usize,
    streams: usize,
    chunk: usize,
    work: Work,
) -> io::Result<()> {
    let (mut fields, mut header) = (Vec::new(), Vec::new());
    for frame in slot_frames(&messages, slot, streams, chunk) {
        write_frame(io, protocol, frame, &mut fields, &mut header, work).await?;
    }
    Ok(())
}

/// The family of a value behind a lock; its view holds the lock.
pub struct Locked<L>(PhantomData<fn() -> L>);
pub struct LockedView<'v, L: Lock + 'v>(pub Held<'v, L>);
impl<L: Lock> ReceiverFamily for Locked<L> {
    type ReceiverView<'v>
        = LockedView<'v, L>
    where
        Self: 'v;
}
impl<L: Lock> Consume for Locked<L>
where
    L::Value: ByteSource,
{
    fn poll_consume<'visit, F: FnMut(&[u8]) + ?Sized>(
        host: Pin<&mut LockedView<'visit, L>>,
        cx: &mut Context<'_>,
        maximum: usize,
        visit: &mut F,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit,
    {
        ByteSource::poll_consume(&mut *host.get_mut().0, cx, maximum, visit)
    }
}
impl<L: Lock> Take for Locked<L>
where
    L::Value: ByteSource,
{
    fn poll_take<'visit>(
        host: Pin<&mut LockedView<'visit, L>>,
        cx: &mut Context<'_>,
        maximum: usize,
    ) -> Poll<io::Result<BytesMut>>
    where
        Self: 'visit,
    {
        ByteSource::poll_take(&mut *host.get_mut().0, cx, maximum)
    }
}
impl<L: Lock> Write for Locked<L>
where
    L::Value: ByteSink,
{
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut LockedView<'visit, L>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit,
    {
        ByteSink::poll_write(&mut *host.get_mut().0, cx, input)
    }
}

/// When the executor gives the view back at a release point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Release {
    /// Never: the view is kept until the end of the poll.
    Never,
    /// At every release point.
    Always,
}

/// Acquires the lock on the polls whose operations ask for the resource, and
/// releases it when the poll ends, or earlier at release points as `release`
/// says.
pub struct Shared<'p, L> {
    pub lock: &'p L,
    pub stats: &'p LockStats,
    pub release: Release,
}

/// Lock attempts for [`Shared`], kept across polls while waiting.
pub struct LockAcquisition<'p, L: Lock> {
    attempt: PollLock<'p, L>,
    release: Release,
}
impl<'p, L: Lock> ViewAcquisition<'p> for LockAcquisition<'p, L> {
    type Family = Locked<L>;
    fn poll_acquire(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Lend<LockedView<'p, L>>> {
        let this = self.get_mut();
        let view = LockedView(ready!(this.attempt.poll_lock(cx)));
        Poll::Ready(match this.release {
            Release::Never => Lend::Fixed(view),
            Release::Always => Lend::Releasable(view),
        })
    }
    fn advance_unrequested(self: Pin<&mut Self>, cx: &mut Context<'_>) {
        let attempt = &mut self.get_mut().attempt;
        if attempt.is_pending() {
            // Granted while the operation waits for something else: release
            // it at once.
            let _ = attempt.poll_lock(cx);
        }
    }
}

type Event<F, Op> = ControlFlow<<Op as Operation<F>>::Output, <Op as Operation<F>>::Yield>;
impl<'p, L: Lock> CallExecutor for Shared<'p, L> {
    type Family = Locked<L>;
    type Execution<'a, Op>
        = impl Future<Output = <Op as Operation<Locked<L>>>::Output>
        + Stream<Item = Event<Locked<L>, Op>>
    where
        Self: 'a,
        Op: Operation<Locked<L>> + for<'s> Start<'s, ReceiverSlot<'s, Locked<L>>> + 'a;
    fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> Self::Execution<'a, Op>
    where
        Op: Operation<Locked<L>> + for<'s> Start<'s, ReceiverSlot<'s, Locked<L>>> + 'a,
    {
        let Shared {
            lock,
            stats,
            release,
        } = *self.get_mut();
        let acquisition = LockAcquisition {
            attempt: PollLock::new(lock, stats),
            release,
        };
        drive(acquisition, operation)
    }
}

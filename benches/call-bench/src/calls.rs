//! `nitori_call` operations over any byte source or sink, and the executor
//! that acquires a lock on the polls whose operations need the resource.
use crate::{
    common::{ByteSink, ByteSource, Lock, Project, bump},
    fields::Fields,
    workload::{Frame, FrameHeader, Kind, Message, Protocol, data_checksum, slot_frames},
};
use bytes::Buf;
use futures_core::Stream;
use nitori_call::{
    AcquisitionState, CallExecutor, Operation, ReceiverAccess, ReceiverFamily, ReceiverSlot, Start,
    call, capability, drive,
};
use std::{
    future::Future,
    io,
    marker::PhantomData,
    ops::ControlFlow,
    pin::Pin,
    sync::{Arc, atomic::AtomicUsize},
    task::{Context, Poll},
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
/// DATA is visited in place.
#[call]
pub async fn read_frame<P: Protocol>(
    io: &mut impl ReceiverAccess<Family: Consume>,
    _protocol: P,
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
            P::decode_fields(block, fields)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
            fields.checksum()
        }
        Kind::Data => {
            let mut checksum = 0;
            consume_exact(io, length, &mut |bytes: &[u8]| {
                checksum += data_checksum(bytes)
            })
            .await?;
            checksum
        }
    }))
}

/// Read every frame and return the total checksum.
#[call]
pub async fn read_all<P: Protocol>(
    io: &mut impl ReceiverAccess<Family: Consume>,
    protocol: P,
    block: &mut Vec<u8>,
    fields: &mut Fields,
) -> io::Result<u64> {
    let mut total = 0;
    while let Some(checksum) = read_frame(io, protocol, block, fields).await? {
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

/// Write one frame: its header, then its payload straight from the message.
#[call]
pub async fn write_frame<P: Protocol>(
    io: &mut impl ReceiverAccess<Family: Write>,
    _protocol: P,
    frame: Frame<'_>,
    fields: &mut Vec<u8>,
    header: &mut Vec<u8>,
) -> io::Result<()> {
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
) -> io::Result<()> {
    let (mut fields, mut header) = (Vec::new(), Vec::new());
    for frame in frames {
        write_frame(io, protocol, frame, &mut fields, &mut header).await?;
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
) -> io::Result<()> {
    let (mut fields, mut header) = (Vec::new(), Vec::new());
    for frame in slot_frames(&messages, slot, streams, chunk) {
        write_frame(io, protocol, frame, &mut fields, &mut header).await?;
    }
    Ok(())
}

/// The family of a part of a value behind a lock; its view holds the lock's
/// guard and selects the part.
pub struct Locked<L, P>(PhantomData<fn() -> (L, P)>);
pub struct LockedView<'v, L: Lock + 'v, P>(pub L::Guard<'v>, pub P);
impl<L: Lock, P: 'static> ReceiverFamily for Locked<L, P> {
    type ReceiverView<'v>
        = LockedView<'v, L, P>
    where
        Self: 'v;
}
impl<L: Lock, P: Project<L::Value>> Consume for Locked<L, P>
where
    P::Target: ByteSource,
{
    fn poll_consume<'visit, F: FnMut(&[u8]) + ?Sized>(
        host: Pin<&mut LockedView<'visit, L, P>>,
        cx: &mut Context<'_>,
        maximum: usize,
        visit: &mut F,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit,
    {
        let LockedView(guard, project) = host.get_mut();
        ByteSource::poll_consume(project.project(guard), cx, maximum, visit)
    }
}
impl<L: Lock, P: Project<L::Value>> Write for Locked<L, P>
where
    P::Target: ByteSink,
{
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut LockedView<'visit, L, P>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit,
    {
        let LockedView(guard, project) = host.get_mut();
        ByteSink::poll_write(project.project(guard), cx, input)
    }
}

/// Acquires the lock on the polls whose operations ask for the resource, and
/// releases it when the poll ends.
pub struct Shared<'p, L, P> {
    pub lock: &'p L,
    pub project: P,
    pub locks: &'p AtomicUsize,
}
type Event<F, Op> = ControlFlow<<Op as Operation<F>>::Output, <Op as Operation<F>>::Yield>;
impl<'p, L: Lock, P: Project<L::Value>> CallExecutor for Shared<'p, L, P> {
    type Family = Locked<L, P>;
    type Execution<'a, Op>
        = impl Future<Output = <Op as Operation<Locked<L, P>>>::Output>
        + Stream<Item = Event<Locked<L, P>, Op>>
    where
        Self: 'a,
        Op: Operation<Locked<L, P>> + for<'s> Start<'s, ReceiverSlot<'s, Locked<L, P>>> + 'a;
    fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> Self::Execution<'a, Op>
    where
        Op: Operation<Locked<L, P>> + for<'s> Start<'s, ReceiverSlot<'s, Locked<L, P>>> + 'a,
    {
        let Shared {
            lock,
            project,
            locks,
        } = *self.get_mut();
        let acquisition = AcquisitionState::<Locked<L, P>, _, _>::new(move || {
            bump(locks);
            async move { LockedView(crate::common::lock(lock).await, project) }
        });
        drive(acquisition, operation)
    }
}

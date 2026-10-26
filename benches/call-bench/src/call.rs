//! `nitori_call`: async `#[call]` over `ReceiverAccess`, with `CallExecutor`
//! and `drive`.
use crate::common::{Source, frame_checksum, poll_read_into};
use bytes::BufMut;
use futures_core::Stream;
use nitori_call::{
    AcquisitionState, BorrowedReceiver, CallExecutor, Operation, ReceiverAccess, ReceiverFamily,
    ReceiverSlot, Start, call, capability, drive, family_receiver,
};
use std::{
    cell::Cell,
    future::{Future, poll_fn},
    io,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll},
};
use tokio::sync::{Mutex, MutexGuard};

#[capability]
pub trait Read: ReceiverFamily {
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit;
}

pub struct Conn(pub Source);
family_receiver!(impl [] for Conn);
impl Read for Conn {
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut BorrowedReceiver<'visit, Conn>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit,
    {
        poll_read_into(
            Pin::new(&mut host.get_mut().0.as_mut().get_mut().0),
            cx,
            destination,
        )
    }
}

pub struct Locked;
pub struct LockedView<'v>(MutexGuard<'v, Source>);
impl ReceiverFamily for Locked {
    type ReceiverView<'v> = LockedView<'v>;
}
impl Read for Locked {
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut LockedView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit,
    {
        poll_read_into(Pin::new(&mut *host.get_mut().0), cx, destination)
    }
}

#[call]
async fn read_exact_buf<B: BufMut + ?Sized>(
    io: &mut impl ReceiverAccess<Family: Read>,
    output: &mut B,
    length: usize,
) -> io::Result<()> {
    let mut done = 0;
    while done < length {
        let mut limited = (&mut *output).limit(length - done);
        let count = io.read(&mut limited).await?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        done += count;
    }
    Ok(())
}

#[call]
pub async fn read_frame(
    io: &mut impl ReceiverAccess<Family: Read>,
    payload: &mut Vec<u8>,
) -> io::Result<u8> {
    let mut tag = [0u8; 1];
    let mut tag_buffer = &mut tag[..];
    read_exact_buf(io, &mut tag_buffer, 1).await?;
    let mut length = [0u8; 2];
    let mut length_buffer = &mut length[..];
    read_exact_buf(io, &mut length_buffer, 2).await?;
    payload.clear();
    read_exact_buf(io, payload, u16::from_be_bytes(length).into()).await?;
    Ok(tag[0])
}

#[call(yields = u64)]
async fn read_frames(io: &mut impl ReceiverAccess<Family: Read>, frames: usize) -> usize {
    let mut payload = Vec::new();
    for _ in 0..frames {
        let tag = read_frame(io, &mut payload).await.unwrap();
        yield frame_checksum(tag, &payload);
    }
    frames
}

/// Acquires the lock on the polls that read.
pub struct Reader<'p> {
    pub shared: &'p Mutex<Source>,
    pub locks: &'p Cell<usize>,
}
type Event<Op> = ControlFlow<<Op as Operation<Locked>>::Output, <Op as Operation<Locked>>::Yield>;
impl<'p> CallExecutor for Reader<'p> {
    type Family = Locked;
    type Execution<'a, Op>
        = impl Future<Output = <Op as Operation<Locked>>::Output> + Stream<Item = Event<Op>>
    where
        Self: 'a,
        Op: Operation<Locked> + for<'s> Start<'s, ReceiverSlot<'s, Locked>> + 'a;
    fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> Self::Execution<'a, Op>
    where
        Op: Operation<Locked> + for<'s> Start<'s, ReceiverSlot<'s, Locked>> + 'a,
    {
        let Reader { shared, locks } = *self.get_mut();
        let acquisition = AcquisitionState::<Locked, _, _>::new(move || {
            locks.set(locks.get() + 1);
            async move { LockedView(shared.lock().await) }
        });
        drive(acquisition, operation)
    }
}

pub async fn exclusive(source: Source, frames: usize) -> u64 {
    let mut conn = Conn(source);
    let mut payload = Vec::new();
    let mut checksum = 0;
    for _ in 0..frames {
        let tag = conn.read_frame_unpin(&mut payload).await.unwrap();
        checksum += frame_checksum(tag, &payload);
    }
    checksum
}

async fn sum_stream<S: Stream<Item = ControlFlow<usize, u64>>>(stream: S, frames: usize) -> u64 {
    let mut stream = pin!(stream);
    let mut checksum = 0;
    loop {
        match poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            Some(ControlFlow::Continue(sum)) => checksum += sum,
            Some(ControlFlow::Break(count)) => {
                assert_eq!(count, frames);
                return checksum;
            }
            None => unreachable!(),
        }
    }
}

pub async fn exclusive_stream(source: Source, frames: usize) -> u64 {
    let mut conn = Conn(source);
    sum_stream(conn.read_frames_unpin(frames), frames).await
}

pub async fn shared(shared: &Mutex<Source>, locks: &Cell<usize>, frames: usize) -> u64 {
    let mut reader = Reader { shared, locks };
    let mut payload = Vec::new();
    let mut checksum = 0;
    for _ in 0..frames {
        let tag = reader.read_frame_unpin(&mut payload).await.unwrap();
        checksum += frame_checksum(tag, &payload);
    }
    checksum
}

pub async fn shared_stream(shared: &Mutex<Source>, locks: &Cell<usize>, frames: usize) -> u64 {
    let mut reader = Reader { shared, locks };
    sum_stream(reader.read_frames_unpin(frames), frames).await
}

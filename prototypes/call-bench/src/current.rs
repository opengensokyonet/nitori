//! The `nitori_call` and `nitori_io` crates: coroutine-lowered `#[call]` with
//! `Target`, `ResourceDriver` and `Execution`.
use crate::common::{Source, frame_checksum, poll_read_into};
use bytes::BufMut;
use futures_core::Stream;
use nitori_call::{
    AcquisitionState, BorrowedReceiver, CallOn, Execution, ExecutionControl, HasReceiverFamily,
    ReceiverFamily, ResourceDriver, Target, call, drive, family_receiver,
};
use nitori_io::{Read, TargetReadExt};
use std::{
    cell::Cell,
    future::{Future, poll_fn},
    io,
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll},
};
use tokio::sync::{Mutex, MutexGuard};

pub struct Conn(pub Source);
family_receiver!(impl [] for Conn);
impl Read for Conn {
    type Error = io::Error;
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
impl HasReceiverFamily for LockedView<'_> {
    type Family = Locked;
}
impl Read for Locked {
    type Error = io::Error;
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
async fn read_exact_buf<'d, H: Read<Error = io::Error>, B: BufMut + ?Sized>(
    io: Target<H>,
    output: &'d mut B,
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
pub async fn read_frame<'d, H: Read<Error = io::Error>>(
    io: Target<H>,
    payload: &'d mut Vec<u8>,
) -> io::Result<u8> {
    let mut tag = [0u8; 1];
    let mut tag_buffer = &mut tag[..];
    io.read_exact_buf(&mut tag_buffer, 1).await?;
    let mut length = [0u8; 2];
    let mut length_buffer = &mut length[..];
    io.read_exact_buf(&mut length_buffer, 2).await?;
    payload.clear();
    io.read_exact_buf(payload, u16::from_be_bytes(length).into())
        .await?;
    Ok(tag[0])
}

#[call(yields = u64)]
async fn read_frames<H: Read<Error = io::Error>>(io: Target<H>, frames: usize) -> usize {
    let mut payload = Vec::new();
    for _ in 0..frames {
        let tag = io.read_frame(&mut payload).await.unwrap();
        yield frame_checksum(tag, &payload);
    }
    frames
}

/// Acquires the lock on the polls that read.
pub struct Reader<'p> {
    pub shared: &'p Mutex<Source>,
    pub locks: &'p Cell<usize>,
}
impl<'p> ResourceDriver for Reader<'p> {
    type Family = Locked;
    type Execution<'a, O>
        = impl Future<Output = O::Return> + Stream<Item = CoroutineState<O::Yield, O::Return>>
    where
        Self: 'a,
        O: CallOn<Locked> + 'a;
    fn execute<'a, O>(self: Pin<&'a mut Self>, operation: O) -> Self::Execution<'a, O>
    where
        O: CallOn<Locked> + 'a,
    {
        let Reader { shared, locks } = *self.get_mut();
        Execution::new(driver_call(shared, locks, operation))
    }
}

#[call(yields = O::Yield)]
async fn driver_call<'p, O: CallOn<Locked>>(
    _control: Target<ExecutionControl>,
    shared: &'p Mutex<Source>,
    locks: &'p Cell<usize>,
    operation: O,
) -> O::Return {
    let acquisition = AcquisitionState::<Locked, _, _>::new(move || {
        locks.set(locks.get() + 1);
        async move { LockedView(shared.lock().await) }
    });
    let mut acquisition = pin!(acquisition);
    let mut operation = pin!(operation);
    loop {
        match drive(acquisition.as_mut(), operation.as_mut()).await {
            CoroutineState::Yielded(value) => yield value,
            CoroutineState::Complete(value) => return value,
        }
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

async fn sum_stream<S: Stream<Item = CoroutineState<u64, usize>>>(stream: S, frames: usize) -> u64 {
    let mut stream = pin!(stream);
    let mut checksum = 0;
    loop {
        match poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            Some(CoroutineState::Yielded(sum)) => checksum += sum,
            Some(CoroutineState::Complete(count)) => {
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

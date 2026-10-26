//! Ordinary async over tokio's `AsyncReadExt`.
use crate::common::{Source, frame_checksum, poll_read_into};
use bytes::BufMut;
use std::{
    cell::Cell,
    future::poll_fn,
    io,
    pin::{Pin, pin},
    task::Poll,
};
use tokio::{io::AsyncReadExt, sync::Mutex};

pub async fn read_frame(source: &mut Source, payload: &mut Vec<u8>) -> io::Result<u8> {
    let tag = source.read_u8().await?;
    let length = source.read_u16().await?;
    payload.resize(length.into(), 0);
    source.read_exact(payload).await?;
    Ok(tag)
}

/// The connection is owned by the reader.
pub async fn exclusive(mut source: Source, frames: usize) -> u64 {
    let mut payload = Vec::new();
    let mut checksum = 0;
    for _ in 0..frames {
        let tag = read_frame(&mut source, &mut payload).await.unwrap();
        checksum += frame_checksum(tag, &payload);
    }
    checksum
}

/// The connection is shared: lock it for each frame, holding the lock while
/// waiting for its bytes.
pub async fn hold_per_frame(shared: &Mutex<Source>, locks: &Cell<usize>, frames: usize) -> u64 {
    let mut payload = Vec::new();
    let mut checksum = 0;
    for _ in 0..frames {
        locks.set(locks.get() + 1);
        let mut source = shared.lock().await;
        let tag = read_frame(&mut source, &mut payload).await.unwrap();
        drop(source);
        checksum += frame_checksum(tag, &payload);
    }
    checksum
}

/// One read under the lock, released before waiting: each poll locks.
async fn locked_read<O: BufMut + ?Sized>(
    shared: &Mutex<Source>,
    locks: &Cell<usize>,
    destination: &mut O,
) -> usize {
    poll_fn(|cx| {
        locks.set(locks.get() + 1);
        let Poll::Ready(mut source) = pin!(shared.lock()).poll(cx) else {
            return Poll::Pending;
        };
        poll_read_into(Pin::new(&mut *source), cx, destination).map(Result::unwrap)
    })
    .await
}
async fn locked_exact(
    shared: &Mutex<Source>,
    locks: &Cell<usize>,
    output: &mut Vec<u8>,
    length: usize,
) {
    let mut done = 0;
    while done < length {
        let count = locked_read(shared, locks, &mut output.limit(length - done)).await;
        assert_ne!(count, 0, "unexpected end");
        done += count;
    }
}

/// The connection is shared and never held while waiting: every leaf read
/// locks on each poll.
pub async fn lock_per_poll(shared: &Mutex<Source>, locks: &Cell<usize>, frames: usize) -> u64 {
    let mut header = Vec::with_capacity(3);
    let mut payload = Vec::new();
    let mut checksum = 0;
    for _ in 0..frames {
        header.clear();
        locked_exact(shared, locks, &mut header, 1).await;
        locked_exact(shared, locks, &mut header, 2).await;
        let length = u16::from_be_bytes([header[1], header[2]]);
        payload.clear();
        locked_exact(shared, locks, &mut payload, length.into()).await;
        checksum += frame_checksum(header[0], &payload);
    }
    checksum
}

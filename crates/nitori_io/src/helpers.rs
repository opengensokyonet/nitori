//! Explicit conversions; neither helper installs a blanket capability impl.
use crate::{Read, ReadChunk};
use bytes::{Buf, BufMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, ready},
};

/// Fill from one chunk, copying its entire contents without retaining a tail.
///
/// Call this after any host-specific terminal-error checks: an empty destination
/// returns zero locally because a chunk request must be nonzero. In particular,
/// this helper must not bypass an error that the host prioritizes over emptiness.
pub fn poll_read_from_chunk<H: ReadChunk + ?Sized, B: BufMut + ?Sized>(
    host: Pin<&mut H>,
    cx: &mut Context<'_>,
    mut destination: &mut B,
) -> Poll<Result<usize, H::Error>> {
    let Some(maximum) = NonZeroUsize::new(destination.remaining_mut()) else {
        return Poll::Ready(Ok(0));
    };
    let Some(chunk) = ready!(host.poll_read_chunk(cx, maximum))? else {
        return Poll::Ready(Ok(0));
    };
    let count = chunk.remaining();
    assert!(count > 0 && count <= maximum.get(), "invalid chunk length");
    BufMut::put(&mut destination, chunk);
    Poll::Ready(Ok(count))
}

/// Read into caller-selected, final chunk storage.
///
/// The caller owns `storage` and retains it across Pending. `create` receives
/// the requested maximum and must return a buffer with at least one writable
/// byte (it may cap allocation below maximum). `finish` exposes exactly the
/// newly written prefix as a chunk. Allocation policy and allocation failures
/// belong to the caller; prepare storage before calling to use fallible creation.
/// On EOF or error storage is released; on success it is passed to `finish`.
/// A new maximum on a later poll is respected. Do not share active storage
/// between unrelated operations. Dropping it on cancellation consumes no bytes.
pub fn poll_chunk_from_read<H, B, C>(
    host: Pin<&mut H>,
    cx: &mut Context<'_>,
    maximum: NonZeroUsize,
    storage: &mut Option<B>,
    create: impl FnOnce(usize) -> B,
    finish: impl FnOnce(B) -> C,
) -> Poll<Result<Option<C>, H::Error>>
where
    H: Read + ?Sized,
    B: BufMut,
    C: Buf,
{
    let buffer = storage.get_or_insert_with(|| create(maximum.get()));
    let limit = maximum.get().min(buffer.remaining_mut());
    assert!(limit > 0, "chunk storage must have writable capacity");
    let result = host.poll_read(cx, &mut buffer.limit(limit));
    match result {
        Poll::Pending => Poll::Pending,
        Poll::Ready(Err(error)) => {
            storage.take();
            Poll::Ready(Err(error))
        }
        Poll::Ready(Ok(0)) => {
            storage.take();
            Poll::Ready(Ok(None))
        }
        Poll::Ready(Ok(count)) => {
            assert!(count <= limit, "read exceeded chunk capacity");
            let chunk = finish(storage.take().unwrap());
            assert_eq!(
                chunk.remaining(),
                count,
                "finish must expose only the read prefix"
            );
            Poll::Ready(Ok(Some(chunk)))
        }
    }
}

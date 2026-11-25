//! Explicit conversions; neither helper installs a blanket capability impl.
use crate::{ChunkSegment, PollReadExt as _, Read, ReadChunk, ReadResult, ReadSegment};
use bytes::{Buf, BufMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::Receiver;

/// Fill from one chunk, copying its entire contents without retaining a tail;
/// the segment has the chunk's metadata.
///
/// Call this after any host-specific terminal-error checks: an empty destination
/// returns zero locally because a chunk request must be nonzero. In particular,
/// this helper must not bypass an error that the host prioritizes over emptiness.
pub fn poll_read_from_chunk<H: Receiver + ?Sized, B: BufMut + ?Sized>(
    host: Pin<&mut H>,
    cx: &mut Context<'_>,
    mut destination: &mut B,
) -> Poll<ReadResult<H::Family>>
where
    H::Family: ReadChunk,
{
    let Some(maximum) = NonZeroUsize::new(destination.remaining_mut()) else {
        return Poll::Ready(Ok(None));
    };
    let Some(ChunkSegment { chunk, metadata }) = ready!(host.poll_read_chunk(cx, maximum))? else {
        return Poll::Ready(Ok(None));
    };
    let count = NonZeroUsize::new(chunk.remaining())
        .filter(|count| *count <= maximum)
        .expect("invalid chunk length");
    BufMut::put(&mut destination, chunk);
    Poll::Ready(Ok(Some(ReadSegment { count, metadata })))
}

/// Read into caller-selected, final chunk storage.
///
/// The caller owns `storage` and retains it across Pending. `create` receives
/// the requested maximum and must return a buffer with at least one writable
/// byte (it may cap allocation below maximum). `finish` exposes exactly the
/// newly written prefix as a chunk, which has the read segment's metadata. Allocation policy and allocation failures
/// belong to the caller; prepare storage before calling to use fallible creation.
/// On EOF or error storage is released; on success it is passed to `finish`.
/// A new maximum on a later poll is respected. Do not share active storage
/// between unrelated operations. Dropping it on cancellation consumes no bytes.
#[allow(clippy::type_complexity)]
pub fn poll_chunk_from_read<H, B, C>(
    host: Pin<&mut H>,
    cx: &mut Context<'_>,
    maximum: NonZeroUsize,
    storage: &mut Option<B>,
    create: impl FnOnce(usize) -> B,
    finish: impl FnOnce(B) -> C,
) -> Poll<Result<Option<ChunkSegment<C, <H::Family as Read>::Metadata>>, <H::Family as Read>::Error>>
where
    H: Receiver + ?Sized,
    H::Family: Read,
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
        Poll::Ready(Ok(None)) => {
            storage.take();
            Poll::Ready(Ok(None))
        }
        Poll::Ready(Ok(Some(ReadSegment { count, metadata }))) => {
            assert!(count.get() <= limit, "read exceeded chunk capacity");
            let chunk = finish(storage.take().unwrap());
            assert_eq!(
                chunk.remaining(),
                count.get(),
                "finish must expose only the read prefix"
            );
            Poll::Ready(Ok(Some(ChunkSegment { chunk, metadata })))
        }
    }
}

//! Skipping built from reading: composition states that give a reader the
//! [`Skip`] capability by reading and dropping what it reads.
use crate::{ExactLen, Read, ReadChunk, Skip};
use bytes::{Buf, BufMut};
use core::{
    marker::PhantomData,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::{Compose, ReceiverFamily};

/// The size of the scratch buffer [`ReadSkip`] reads into on each poll.
///
/// It matches the stack buffer of the bridge reads: skipping through a bridge
/// then takes no more polls than the bridge itself, and the buffer lives on
/// one poll's stack, not in a future.
const SCRATCH_LEN: usize = 8192;

/// Composition state that skips by reading: each skip reads at most 8 KiB into
/// a stack buffer on that poll and drops it. Compose it on any family with
/// [`Read`], either on an access, `io.compose(ReadSkip)`, or on a receiver,
/// `ComposedReceiver::new(receiver, ReadSkip)`. It holds nothing, so it
/// counts nothing either: the reads report what was consumed, and a count kept
/// above it stays in step across cancellation.
///
/// The skipped `Count` is `usize`. A zero maximum makes an empty read, which
/// reaches the parent. A source that skips natively should implement [`Skip`]
/// instead, and one that produces chunks can use [`ReadChunkSkip`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ReadSkip;

/// Composition state that skips by chunk reads: each skip takes one chunk of
/// at most the maximum and drops it, without copying. Compose it on any family
/// with [`ReadChunk`], on an access or on a receiver, like [`ReadSkip`].
///
/// The skipped `Count` is `usize`; a maximum above `usize::MAX`, or
/// [`Infinite`](crate::Infinite), requests a chunk of at most `usize::MAX`.
/// A zero maximum cannot be a chunk request, so it makes an empty read on the
/// parent instead, which can still report its terminal error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ReadChunkSkip;

/// The family of a view of `F` composed with [`ReadSkip`] or
/// [`ReadChunkSkip`] (`S`): its views are the views of `F` themselves. It
/// forwards [`Read`], [`ReadChunk`] and [`ExactLen`] when `F` has them, and
/// skips through `S`.
pub struct Skippable<F, S>(PhantomData<fn(F) -> F>, PhantomData<S>);

impl<F: ReceiverFamily, S> ReceiverFamily for Skippable<F, S> {
    type ReceiverView<'v>
        = F::ReceiverView<'v>
    where
        Self: 'v;
}

macro_rules! skip_compose {
    ($($state:ty),*) => {$(
        impl<F: ReceiverFamily> Compose<F> for $state {
            type Family = Skippable<F, Self>;
            fn compose<'v>(self: Pin<&'v mut Self>, parent: F::ReceiverView<'v>) -> F::ReceiverView<'v>
            where
                F: 'v,
                Self::Family: 'v,
            {
                parent
            }
        }
    )*};
}
skip_compose!(ReadSkip, ReadChunkSkip);

impl<F: Read, S> Read for Skippable<F, S> {
    type Error = F::Error;
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'v,
    {
        F::poll_read(host, cx, destination)
    }
}
impl<F: ReadChunk, S> ReadChunk for Skippable<F, S> {
    type Chunk = F::Chunk;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<F::Chunk>, Self::Error>>
    where
        Self: 'v,
    {
        F::poll_read_chunk(host, cx, maximum)
    }
}
impl<F: ExactLen, S> ExactLen for Skippable<F, S> {
    type Length = F::Length;
    fn remaining_len<'v>(host: Pin<&mut Self::ReceiverView<'v>>) -> F::Length
    where
        Self: 'v,
    {
        F::remaining_len(host)
    }
}

/// Reads into scratch storage and drops what was read.
impl<F: Read> Skip for Skippable<F, ReadSkip> {
    type Count = usize;
    fn poll_skip<'v, M: crate::Min<usize, Output = usize> + Clone>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<usize>, Self::Error>>
    where
        Self: 'v,
    {
        let mut scratch = [0; SCRATCH_LEN];
        let allowed = crate::Min::min(maximum, SCRATCH_LEN);
        let count = ready!(F::poll_read(host, cx, &mut &mut scratch[..allowed]))?;
        assert!(count <= allowed, "read exceeded the requested capacity");
        // An empty read cannot report EOF; only a nonempty one can.
        Poll::Ready(Ok((allowed == 0 || count != 0).then_some(count)))
    }
}

/// Takes one chunk and drops it.
impl<F: ReadChunk> Skip for Skippable<F, ReadChunkSkip> {
    type Count = usize;
    fn poll_skip<'v, M: crate::Min<usize, Output = usize> + Clone>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<usize>, Self::Error>>
    where
        Self: 'v,
    {
        let Some(allowed) = NonZeroUsize::new(crate::Min::min(maximum, usize::MAX)) else {
            let mut empty: &mut [u8] = &mut [];
            ready!(F::poll_read(host, cx, &mut empty))?;
            return Poll::Ready(Ok(Some(0)));
        };
        let chunk = ready!(F::poll_read_chunk(host, cx, allowed))?;
        Poll::Ready(Ok(chunk.map(|chunk| {
            let count = chunk.remaining();
            assert!(
                count > 0 && count <= allowed.get(),
                "invalid read chunk length"
            );
            count
        })))
    }
}

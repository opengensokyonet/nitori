//! Reading with the metadata merged into composition state, for values that
//! read from a source without metadata.
use crate::{ChunkResult, ChunkSegment, Combine, Read, ReadChunk, ReadResult, ReadSegment, Skip};
use bytes::BufMut;
use core::{
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::{AsAccess, BorrowedAccess, Compose, Composed, Lent, ReceiverFamily};

/// The merged metadata of the reads made through a composed access:
/// `io.compose(MergedMetadata::default())` reads from `io` and merges the
/// metadata of each segment into the state with [`Combine`], while the
/// composed access reports none (`Metadata = ()`). A value that reads only
/// from sources without metadata can then read from one with it, and the
/// merged metadata of its bytes stays here; `into_state` returns it.
///
/// Chunk reads, skipping and the exact length are forwarded. Skipping does
/// not report metadata, so the metadata of skipped bytes does not reach the
/// state. Kept with a
/// receiver in a [`ComposedReceiver`](nitori_call::ComposedReceiver), the
/// state merges the reads of every call on that receiver.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MergedMetadata<M>(pub M);

impl<M: Combine> MergedMetadata<M> {
    fn merge(&mut self, next: M) {
        self.0 = core::mem::take(&mut self.0).combine(next);
    }
}

/// The family of a view composed with [`MergedMetadata`] on a view of `F`.
pub struct Merging<F, M>(PhantomData<fn(F) -> F>, PhantomData<M>);
pin_project_lite::pin_project! {
    /// The view of a [`Merging`] family: the parent view, held by value, and
    /// the merged metadata.
    pub struct MergingView<'v, F, M>
    where
        F: ReceiverFamily,
        F: 'v,
    {
        #[pin]
        parent: F::ReceiverView<'v>,
        merged: &'v mut MergedMetadata<M>,
    }
}
impl<F: ReceiverFamily, M: fmt::Debug> fmt::Debug for MergingView<'_, F, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MergingView")
            .field("merged", &self.merged.0)
            .finish_non_exhaustive()
    }
}
impl<F: ReceiverFamily, M> ReceiverFamily for Merging<F, M> {
    type ReceiverView<'v>
        = MergingView<'v, F, M>
    where
        Self: 'v;
}
impl<F: ReceiverFamily, M: Unpin> Compose<F> for MergedMetadata<M> {
    type Family = Merging<F, M>;
    fn compose<'v>(self: Pin<&'v mut Self>, parent: F::ReceiverView<'v>) -> MergingView<'v, F, M>
    where
        F: 'v,
        Self::Family: 'v,
    {
        MergingView {
            parent,
            merged: self.get_mut(),
        }
    }
}

/// Reads from the parent and merges each segment's metadata into the state.
impl<F: Read<Metadata = M>, M: Combine> Read for Merging<F, M> {
    type Error = F::Error;
    type Metadata = ();
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<ReadResult<Self>>
    where
        Self: 'v,
    {
        let view = host.project();
        let segment = ready!(F::poll_read(view.parent, cx, destination))?;
        Poll::Ready(Ok(segment.map(|ReadSegment { count, metadata }| {
            view.merged.merge(metadata);
            ReadSegment {
                count,
                metadata: (),
            }
        })))
    }
}
/// Takes the parent's chunks and merges each chunk's metadata into the
/// state.
impl<F: ReadChunk<Metadata = M>, M: Combine> ReadChunk for Merging<F, M> {
    type Chunk = F::Chunk;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<ChunkResult<Self>>
    where
        Self: 'v,
    {
        let view = host.project();
        let segment = ready!(F::poll_read_chunk(view.parent, cx, maximum))?;
        Poll::Ready(Ok(segment.map(|ChunkSegment { chunk, metadata }| {
            view.merged.merge(metadata);
            ChunkSegment {
                chunk,
                metadata: (),
            }
        })))
    }
}
impl<F: Skip<Metadata = M>, M: Combine> Skip for Merging<F, M> {
    type Count = F::Count;
    fn poll_skip<'v, C: crate::Min<F::Count, Output = F::Count> + Clone>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: C,
    ) -> Poll<Result<Option<F::Count>, Self::Error>>
    where
        Self: 'v,
    {
        F::poll_skip(host.project().parent, cx, maximum)
    }
}
impl<F: crate::ExactLen, M> crate::ExactLen for Merging<F, M> {
    type Length = F::Length;
    type LengthError = F::LengthError;
    fn poll_remaining_len<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<F::Length, F::LengthError>>
    where
        Self: 'v,
    {
        F::poll_remaining_len(host.project().parent, cx)
    }
}

/// A source that merges into a [`MergedMetadata`] the metadata of what is
/// read through it: the access it lends is the one `source` lends, composed
/// with the state. It stands for any [`AsAccess`] source, an access or a type
/// that lends one, so a value that reads only from sources without metadata
/// can be decoded from one with it.
#[derive(Debug)]
pub struct MergingSource<'x, S: AsAccess, M> {
    access: Composed<BorrowedAccess<'x, S::Access>, &'x mut MergedMetadata<M>>,
}
impl<'x, S: AsAccess, M> MergingSource<'x, S, M> {
    /// Read from `source`, merging the metadata into `merged`.
    pub fn new(source: &'x mut S, merged: &'x mut MergedMetadata<M>) -> Self {
        Self {
            access: Composed::new(source.access(), merged),
        }
    }
}
impl<'x, S: AsAccess, M: Unpin> AsAccess for MergingSource<'x, S, M> {
    type Family = Merging<Lent<S::Family>, M>;
    type Access = Composed<BorrowedAccess<'x, S::Access>, &'x mut MergedMetadata<M>>;
    fn access(&mut self) -> BorrowedAccess<'_, Self::Access> {
        BorrowedAccess::new(&mut self.access)
    }
}

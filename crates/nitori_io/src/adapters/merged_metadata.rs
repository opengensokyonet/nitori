//! Combine the metadata of successful reads.
use crate::{ChunkSegment, Combine, ExactLen, Read, ReadChunk, ReadSegment, Skip};
use core::{
    borrow::BorrowMut,
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::{
    AccessPlan, Lent, ProvideAccess, Receiver, ReceiverAccess, ReceiverFamily, ViewProjection,
    project_access,
};

type ReadPoll<E, M> = Poll<Result<Option<ReadSegment<M>>, E>>;
type ChunkPoll<C, E, M> = Poll<Result<Option<ChunkSegment<C, M>>, E>>;

pin_project_lite::pin_project! {
    /// A source that combines the metadata from successful reads and reports `()`.
    ///
    /// Read and chunk metadata are combined in order with [`Combine`]. Skips
    /// and length queries pass through without updating the accumulated metadata.
    /// The accumulated metadata persists between calls on a wrapped receiver.
    pub struct MergedMetadata<IO, M, B = M> {
        #[pin]
        io: IO,
        merged: B,
        metadata: PhantomData<fn() -> M>,
    }
}

impl<IO, M> MergedMetadata<IO, M> {
    /// Wrap `io` with an owned accumulator. Start with its `Default` value
    /// to accumulate only the metadata read through this wrapper.
    pub const fn new(io: IO, merged: M) -> Self {
        Self {
            io,
            merged,
            metadata: PhantomData,
        }
    }
}

impl<'m, IO, M> MergedMetadata<IO, M, &'m mut M> {
    /// Wrap `io`, combining metadata into the caller's accumulator.
    pub const fn borrowing(io: IO, merged: &'m mut M) -> Self {
        Self {
            io,
            merged,
            metadata: PhantomData,
        }
    }
}

impl<IO, M, B> MergedMetadata<IO, M, B> {
    fn io_pin(self: Pin<&mut Self>) -> Pin<&mut IO> {
        self.project().io
    }
    /// Borrow the inner IO and the accumulated metadata or its borrow.
    pub const fn get_ref(&self) -> (&IO, &B) {
        (&self.io, &self.merged)
    }
    /// Return the inner IO and the accumulated metadata or its borrow.
    pub fn into_parts(self) -> (IO, B) {
        (self.io, self.merged)
    }
    fn poll_read_with<O: ?Sized, E>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut O,
        read: impl FnOnce(Pin<&mut IO>, &mut Context<'_>, &mut O) -> ReadPoll<E, M>,
    ) -> ReadPoll<E, ()>
    where
        B: BorrowMut<M>,
        M: Combine,
    {
        let this = self.project();
        let segment = ready!(read(this.io, cx, output))?;
        Poll::Ready(Ok(segment.map(|ReadSegment { count, metadata }| {
            merge(this.merged.borrow_mut(), metadata);
            ReadSegment {
                count,
                metadata: (),
            }
        })))
    }
    fn poll_read_chunk_with<C, E>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
        read: impl FnOnce(Pin<&mut IO>, &mut Context<'_>, NonZeroUsize) -> ChunkPoll<C, E, M>,
    ) -> ChunkPoll<C, E, ()>
    where
        B: BorrowMut<M>,
        M: Combine,
    {
        let this = self.project();
        let segment = ready!(read(this.io, cx, maximum))?;
        Poll::Ready(Ok(segment.map(|ChunkSegment { chunk, metadata }| {
            merge(this.merged.borrow_mut(), metadata);
            ChunkSegment {
                chunk,
                metadata: (),
            }
        })))
    }
}

impl<IO, M, B: fmt::Debug> fmt::Debug for MergedMetadata<IO, M, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MergedMetadata")
            .field("merged", &self.merged)
            .finish_non_exhaustive()
    }
}

fn merge<M: Combine>(into: &mut M, next: M) {
    *into = core::mem::take(into).combine(next);
}

/// The family of a view of `F` that accumulates metadata of type `M`.
pub struct Merging<F, M>(PhantomData<fn(F) -> F>, PhantomData<M>);

impl<F: ReceiverFamily, M> ReceiverFamily for Merging<F, M> {
    type ReceiverView<'v>
        = MergedMetadata<F::ReceiverView<'v>, M, &'v mut M>
    where
        Self: 'v;
}

struct MergeView<M>(PhantomData<M>);

impl<F: ReceiverFamily, M> ViewProjection<F> for MergeView<M> {
    type State = M;
    type Family = Merging<F, M>;
    fn project<'v>(
        view: F::ReceiverView<'v>,
        state: &'v mut Self::State,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v,
    {
        MergedMetadata::borrowing(view, state)
    }
}

impl<R: Receiver, M, B: BorrowMut<M>> Receiver for MergedMetadata<R, M, B> {
    type Family = Merging<R::Family, M>;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        Self::Family: 'v,
    {
        let this = self.project();
        <MergeView<M> as ViewProjection<R::Family>>::project(
            this.io.view(),
            this.merged.borrow_mut(),
        )
    }
}

impl<A: ReceiverAccess, M, B: BorrowMut<M>> ProvideAccess for MergedMetadata<A, M, B> {
    type AccessFamily = Merging<Lent<A::Family>, M>;
    type Lentness = <A as ProvideAccess>::Lentness;
    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        project_access::<_, MergeView<M>>(&mut self.io, self.merged.borrow_mut())
    }
}

forward!(Read for [F: Read<Metadata = M>, M: Combine] Merging<F, M>, inner F, Metadata = (), with poll_read_with);
forward!(ReadChunk for [F: ReadChunk<Metadata = M>, M: Combine] Merging<F, M>, inner F, with poll_read_chunk_with);
forward!(Skip for [F: Skip<Metadata = M>, M: Combine] Merging<F, M>, inner F, pass);
forward!(ExactLen for [F: ExactLen, M] Merging<F, M>, inner F, pass);

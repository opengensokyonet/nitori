//! Explicit capability adapters: chunk reads with caller-selected allocation
//! limits, byte budgets for composed accesses, and skipping built from reads.
use crate::{
    ChunkResult, ChunkSegment, LentBuffer, LentOutcome, PollReadExt as _, PollWriteExt as _, Read,
    ReadChunk, ReadResult, Skip, Write, WriteLent, WriteLentResult, WriteResult,
    helpers::poll_chunk_from_read,
};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use core::{
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::{Compose, Direct, Lent, Receiver, ReceiverFamily};

mod skip;
pub use skip::{ReadChunkSkip, ReadSkip, Skippable};

pin_project_lite::pin_project! {
    /// Add copying chunk reads to any reader, with a shared underlying cursor.
    ///
    /// Each allocation requests at most `chunk_capacity` bytes (an allocator may
    /// round up). Pending retains empty storage, replacing it if a later larger
    /// request needs more capacity. Successful reads transfer its ownership. EOF and
    /// errors release it. Ordinary reads and writes pass through
    /// without prefetching, so alternating operations cannot hide an unread tail.
    /// Dropping the adapter releases scratch storage without consuming more input.
    /// Allocation uses the ordinary infallible `BytesMut` allocation policy.
    #[derive(Debug)]
    pub struct Chunked<T> {
        #[pin]
        inner: T,
        chunk_capacity: NonZeroUsize,
        storage: Option<BytesMut>,
    }
}
impl<T> Chunked<T> {
    pub fn new(inner: T, chunk_capacity: NonZeroUsize) -> Self {
        Self {
            inner,
            chunk_capacity,
            storage: None,
        }
    }
    pub fn get_ref(&self) -> &T {
        &self.inner
    }
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
    pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut T> {
        self.project().inner
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
    pub fn chunk_capacity(&self) -> NonZeroUsize {
        self.chunk_capacity
    }
}
impl<T: Receiver> Read for Direct<Chunked<T>>
where
    T::Family: Read,
{
    type Error = <T::Family as Read>::Error;
    type Metadata = <T::Family as Read>::Metadata;
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<ReadResult<Self>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        this.project().inner.poll_read(cx, destination)
    }
}
impl<T: Receiver> ReadChunk for Direct<Chunked<T>>
where
    T::Family: Read,
{
    type Chunk = Bytes;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<ChunkResult<Self>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        let this = this.project();
        let maximum = maximum.min(*this.chunk_capacity);
        // Avoid geometric growth beyond the configured allocation limit when
        // a cancelled/suspended request is followed by a larger one.
        if this
            .storage
            .as_ref()
            .is_some_and(|storage| storage.capacity() < maximum.get())
        {
            *this.storage = None;
        }
        poll_chunk_from_read(
            this.inner,
            cx,
            maximum,
            this.storage,
            BytesMut::with_capacity,
            BytesMut::freeze,
        )
    }
}
/// Skips on the inner source. Chunk storage holds no data between polls, so
/// nothing buffered is passed over or left behind.
impl<T: Receiver> Skip for Direct<Chunked<T>>
where
    T::Family: Skip,
{
    type Count = <T::Family as Skip>::Count;
    fn poll_skip<'visit, M: crate::Min<Self::Count, Output = Self::Count> + Clone>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<Self::Count>, Self::Error>>
    where
        Self: 'visit,
    {
        let inner = host.get_pin_mut().project().inner;
        <T::Family as Skip>::poll_skip(core::pin::pin!(inner.view()), cx, maximum)
    }
}
impl<T: Receiver> crate::ExactLen for Direct<Chunked<T>>
where
    T::Family: crate::ExactLen,
{
    type Length = <T::Family as crate::ExactLen>::Length;
    type LengthError = <T::Family as crate::ExactLen>::LengthError;
    fn poll_remaining_len<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Length, Self::LengthError>>
    where
        Self: 'v,
    {
        let inner = host.get_pin_mut().project().inner;
        <T::Family as crate::ExactLen>::poll_remaining_len(core::pin::pin!(inner.view()), cx)
    }
}
impl<T: Receiver> Write for Direct<Chunked<T>>
where
    T::Family: Write,
{
    type Error = <T::Family as Write>::Error;
    type Report = <T::Family as Write>::Report;
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<WriteResult<Self>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        this.project().inner.poll_write(cx, input)
    }
}
impl<T: Receiver> WriteLent for Direct<Chunked<T>>
where
    T::Family: WriteLent,
{
    type Buffer<'l> = <T::Family as WriteLent>::Buffer<'l>;
    fn poll_write_lent<'visit, U, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<WriteLentResult<Self, U, A>>
    where
        Self: 'visit,
        Fill: for<'l> FnOnce(&mut LentBuffer<Self::Buffer<'l>>) -> Result<U, A>,
    {
        let this = host.get_pin_mut();
        this.project().inner.poll_write_lent(cx, wanted, fill)
    }
}

nitori_call::direct_receiver!(impl [T] for Chunked<T>);

/// A read budget for a composed access: `io.compose(ReadLimit(n))` reads at
/// most `n` more bytes from `io`, then reports EOF, so a child operation
/// cannot read past a record boundary. The budget is retained across polls;
/// `into_state` returns what remains. It composes on any family, so it can
/// also be kept with a receiver in a [`ComposedReceiver`], where it outlives
/// each call on that receiver.
///
/// At the boundary the composed access answers EOF itself, without polling the
/// parent; within the budget, requests (empty ones included) reach the parent.
/// The quota type is independent of the size of one read. If the parent
/// implements [`crate::ExactLen`] and its length can be bounded by this
/// quota, the composed access reports their minimum. Use a typed literal
/// such as `ReadLimit(6u8)` when there is no expected quota type.
///
/// [`ComposedReceiver`]: nitori_call::ComposedReceiver
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimit<L = usize>(pub L);

/// A write budget for a composed access: `io.compose(WriteLimit(n))` accepts
/// at most `n` more bytes, then accepts none, which [`write_all`] reports as
/// zero progress. The budget is retained across polls; `into_state` returns
/// what remains. Like [`ReadLimit`], it can be kept with a receiver in a
/// [`ComposedReceiver`].
///
/// At the boundary the composed access accepts nothing itself, without polling
/// the parent; within the budget, requests (empty ones included) reach the
/// parent. Only [`Write`] and [`WriteLent`] are available through the
/// composed access.
///
/// [`write_all`]: crate::calls::write_all
/// [`ComposedReceiver`]: nitori_call::ComposedReceiver
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteLimit(pub usize);

/// The family of a view composed with [`ReadLimit`] or [`WriteLimit`] on a
/// view of `F`.
pub struct Limited<F, L>(PhantomData<fn(F) -> F>, PhantomData<L>);
pin_project_lite::pin_project! {
    /// The view of a [`Limited`] family: the parent view, held by value, and
    /// the remaining budget.
    pub struct LimitedView<'v, F, L>
    where
        F: ReceiverFamily,
        F: 'v,
    {
        #[pin]
        parent: F::ReceiverView<'v>,
        limit: &'v mut L,
    }
}
impl<F: ReceiverFamily, L: fmt::Debug> fmt::Debug for LimitedView<'_, F, ReadLimit<L>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LimitedView")
            .field("remaining", &self.limit.0)
            .finish_non_exhaustive()
    }
}

impl<F: ReceiverFamily> fmt::Debug for LimitedView<'_, F, WriteLimit> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LimitedView")
            .field("remaining", &self.limit.0)
            .finish_non_exhaustive()
    }
}

impl<F: ReceiverFamily, L> ReceiverFamily for Limited<F, L> {
    type ReceiverView<'v>
        = LimitedView<'v, F, L>
    where
        Self: 'v;
}
macro_rules! limit_compose {
    ($limit:ty $(, $param:ident)*) => {
        impl<F: ReceiverFamily $(, $param: Unpin)*> Compose<F> for $limit {
            type Family = Limited<F, Self>;
            fn compose<'v>(
                self: Pin<&'v mut Self>, parent: F::ReceiverView<'v>,
            ) -> LimitedView<'v,F,Self>
            where F:'v, Self::Family:'v {
                LimitedView {parent, limit:self.get_mut()}
            }
        }
    };
}
limit_compose!(ReadLimit<L>, L);
limit_compose!(WriteLimit);

impl<F: crate::ExactLen, L: Clone> crate::ExactLen for Limited<F, ReadLimit<L>>
where
    F::Length: crate::Min<L>,
{
    type Length = <F::Length as crate::Min<L>>::Output;
    type LengthError = F::LengthError;
    fn poll_remaining_len<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Length, Self::LengthError>>
    where
        Self: 'v,
    {
        let view = host.project();
        let parent = ready!(F::poll_remaining_len(view.parent, cx))?;
        Poll::Ready(Ok(crate::Min::min(parent, view.limit.0.clone())))
    }
}

impl<L> ReadLimit<L>
where
    L: Clone + core::ops::SubAssign<<usize as crate::Min<L>>::Output>,
    usize: crate::Min<L>,
{
    fn consume(&mut self, count: usize) {
        // The host returned at most the allowed count. Taking the minimum
        // therefore preserves that count, in the quota's subtraction type.
        self.0 -= crate::Min::min(count, self.0.clone());
    }
}

impl<F: Read, L> Read for Limited<F, ReadLimit<L>>
where
    L: Clone
        + crate::Min<usize, Output = usize>
        + core::ops::SubAssign<<usize as crate::Min<L>>::Output>,
    usize: crate::Min<L>,
{
    type Error = F::Error;
    type Metadata = F::Metadata;
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<ReadResult<Self>>
    where
        Self: 'v,
    {
        let view = host.project();
        let remaining = crate::Min::min(view.limit.0.clone(), usize::MAX);
        if remaining == 0 {
            return Poll::Ready(Ok(None));
        }
        let allowed = remaining.min(destination.remaining_mut());
        let segment = ready!(F::poll_read(
            view.parent,
            cx,
            &mut destination.limit(allowed)
        ))?;
        if let Some(segment) = &segment {
            let count = segment.count.get();
            assert!(count <= allowed, "read exceeded the requested capacity");
            view.limit.consume(count);
        }
        Poll::Ready(Ok(segment))
    }
}
impl<F: ReadChunk, L> ReadChunk for Limited<F, ReadLimit<L>>
where
    L: Clone
        + crate::Min<usize, Output = usize>
        + core::ops::SubAssign<<usize as crate::Min<L>>::Output>,
    usize: crate::Min<L>,
{
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
        let Some(allowed) = NonZeroUsize::new(crate::Min::min(view.limit.0.clone(), maximum.get()))
        else {
            return Poll::Ready(Ok(None));
        };
        let chunk = ready!(F::poll_read_chunk(view.parent, cx, allowed))?;
        if let Some(ChunkSegment { chunk, .. }) = &chunk {
            let count = chunk.remaining();
            assert!(
                count > 0 && count <= allowed.get(),
                "invalid read chunk length"
            );
            view.limit.consume(count);
        }
        Poll::Ready(Ok(chunk))
    }
}
/// The smaller of a caller's maximum and a budget, as one maximum for the
/// parent.
#[derive(Clone)]
struct BothMaxima<M, L>(M, L);
impl<C, M: crate::Min<C, Output = C>, L: crate::Min<C, Output = C>> crate::Min<C>
    for BothMaxima<M, L>
{
    type Output = C;
    fn min(self, rhs: C) -> C {
        crate::Min::min(self.0, crate::Min::min(self.1, rhs))
    }
}
/// Skips within the budget, reporting EOF at its boundary without polling the
/// parent, as reads do. The parent bounds each skip by both the requested
/// maximum and the budget, so the skipped count, in the parent's `Count`,
/// fits the budget.
///
/// # Panics
///
/// If the parent reports more than both maxima allowed it, as reads do.
impl<F: Skip, L> Skip for Limited<F, ReadLimit<L>>
where
    L: Clone
        + crate::Min<usize, Output = usize>
        + core::ops::SubAssign<<usize as crate::Min<L>>::Output>
        + crate::Min<F::Count, Output = F::Count>
        + core::ops::SubAssign<<F::Count as crate::Min<L>>::Output>,
    usize: crate::Min<L>,
    F::Count: crate::Min<L> + Clone + PartialOrd,
{
    type Count = F::Count;
    fn poll_skip<'v, M: crate::Min<F::Count, Output = F::Count> + Clone>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<F::Count>, Self::Error>>
    where
        Self: 'v,
    {
        let view = host.project();
        let budget = view.limit.0.clone();
        if crate::Min::min(budget.clone(), usize::MAX) == 0 {
            return Poll::Ready(Ok(None));
        }
        let allowed = BothMaxima(maximum, budget);
        let skipped = ready!(F::poll_skip(view.parent, cx, allowed.clone()))?;
        if let Some(count) = &skipped {
            assert!(
                *count <= crate::Min::min(allowed, count.clone()),
                "skip exceeded the requested maximum"
            );
            // The count is at most the budget, so the minimum is the count
            // itself, in the budget's subtraction type.
            view.limit.0 -= crate::Min::min(count.clone(), view.limit.0.clone());
        }
        Poll::Ready(Ok(skipped))
    }
}
impl<F: Write> Write for Limited<F, WriteLimit> {
    type Error = F::Error;
    type Report = F::Report;
    fn poll_write<'v, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<WriteResult<Self>>
    where
        Self: 'v,
    {
        let view = host.project();
        let remaining = view.limit.0;
        // The boundary accepts nothing, without polling the parent.
        if remaining == 0 {
            return Poll::Ready(Ok(None));
        }
        let segment = ready!(F::poll_write(
            view.parent,
            cx,
            &mut (&mut *input).take(remaining)
        ))?;
        if let Some(segment) = &segment {
            let count = segment.count.get();
            assert!(count <= remaining, "write exceeded the offered input");
            view.limit.0 -= count;
        }
        Poll::Ready(Ok(segment))
    }
}

/// Lends at most the remaining budget of the parent's storage. At the
/// boundary it accepts no byte itself, without polling the parent.
impl<F: WriteLent> WriteLent for Limited<F, WriteLimit> {
    type Buffer<'l> = F::Buffer<'l>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<WriteLentResult<Self, T, A>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<Self::Buffer<'l>>) -> Result<T, A>,
    {
        let view = host.project();
        let Some(remaining) = NonZeroUsize::new(view.limit.0) else {
            return Poll::Ready(Ok(None));
        };
        let allowed = wanted.min(remaining);
        let result = ready!(F::poll_write_lent(view.parent, cx, allowed, fill))?;
        if let Some(LentOutcome::Kept(segment)) = &result {
            let count = segment.count.get();
            assert!(count <= allowed.get(), "write exceeded the granted count");
            view.limit.0 -= count;
        }
        Poll::Ready(Ok(result))
    }
}

// A loan of a view has the capabilities of the view it lends, so that state
// composed on an access, which composes on `Lent`, can use them.

/// Reads from the lent view.
impl<F: Read> Read for Lent<F> {
    type Error = F::Error;
    type Metadata = F::Metadata;
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<ReadResult<Self>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_read(parent.into_pin(), cx, destination))
    }
}
/// Reads chunks from the lent view.
impl<F: ReadChunk> ReadChunk for Lent<F> {
    type Chunk = F::Chunk;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<ChunkResult<Self>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_read_chunk(parent.into_pin(), cx, maximum))
    }
}
/// Writes to the lent view.
impl<F: Write> Write for Lent<F> {
    type Error = F::Error;
    type Report = F::Report;
    fn poll_write<'v, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<WriteResult<Self>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_write(parent.into_pin(), cx, input))
    }
}
/// Lends the storage of the lent view.
impl<F: WriteLent> WriteLent for Lent<F> {
    type Buffer<'l> = F::Buffer<'l>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<WriteLentResult<Self, T, A>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<Self::Buffer<'l>>) -> Result<T, A>,
    {
        host.get_mut()
            .with(|parent| F::poll_write_lent(parent.into_pin(), cx, wanted, fill))
    }
}
/// Skips on the lent view.
impl<F: Skip> Skip for Lent<F> {
    type Count = F::Count;
    fn poll_skip<'v, M: crate::Min<F::Count, Output = F::Count> + Clone>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<F::Count>, Self::Error>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_skip(parent.into_pin(), cx, maximum))
    }
}
/// The exact length of the lent view.
impl<F: crate::ExactLen> crate::ExactLen for Lent<F> {
    type Length = F::Length;
    type LengthError = F::LengthError;
    fn poll_remaining_len<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<F::Length, F::LengthError>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_remaining_len(parent.into_pin(), cx))
    }
}

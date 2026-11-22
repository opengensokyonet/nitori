//! Explicit capability adapters: chunk reads with caller-selected allocation
//! limits, and byte budgets for composed accesses.
use crate::{
    PollReadExt as _, PollWriteExt as _, Read, ReadChunk, Write, helpers::poll_chunk_from_read,
};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use core::{
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::{Compose, Direct, Receiver, ReceiverFamily, ViewLoan};

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
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<Result<usize, Self::Error>>
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
    ) -> Poll<Result<Option<Bytes>, Self::Error>>
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
impl<T: Receiver> crate::ExactLen for Direct<Chunked<T>>
where
    T::Family: crate::ExactLen,
{
    type Length = <T::Family as crate::ExactLen>::Length;
    fn remaining_len<'v>(host: Pin<&mut Self::ReceiverView<'v>>) -> Self::Length
    where
        Self: 'v,
    {
        let inner = host.get_pin_mut().project().inner;
        <T::Family as crate::ExactLen>::remaining_len(core::pin::pin!(inner.view()))
    }
}
impl<T: Receiver> Write for Direct<Chunked<T>>
where
    T::Family: Write,
{
    type Error = <T::Family as Write>::Error;
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        this.project().inner.poll_write(cx, input)
    }
}

nitori_call::direct_receiver!(impl [T] for Chunked<T>);

/// A read budget for a composed access: `io.compose(ReadLimit(n))` reads at
/// most `n` more bytes from `io`, then reports EOF, so a child operation
/// cannot read past a record boundary. The budget is retained across polls;
/// `into_state` returns what remains.
///
/// At the boundary the composed access answers EOF itself, without polling the
/// parent; within the budget, requests (empty ones included) reach the parent.
/// The quota type is independent of the size of one read. If the parent
/// implements [`crate::ExactLen`] and its length can be bounded by this
/// quota, the composed access reports their minimum. Use a typed literal
/// such as `ReadLimit(6u8)` when there is no expected quota type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimit<L = usize>(pub L);

/// A write budget for a composed access: `io.compose(WriteLimit(n))` accepts
/// at most `n` more bytes, then accepts none, which [`write_all`] reports as
/// zero progress. The budget is retained across polls; `into_state` returns
/// what remains.
///
/// At the boundary the composed access accepts nothing itself, without polling
/// the parent; within the budget, requests (empty ones included) reach the
/// parent. Only [`Write`] is available through the composed access.
///
/// [`write_all`]: crate::calls::write_all
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteLimit(pub usize);

/// The family of an access composed with [`ReadLimit`] or [`WriteLimit`].
pub struct Limited<F, L>(PhantomData<fn(F) -> F>, PhantomData<L>);
/// A view of a parent view with the remaining budget of a [`Limited`] family.
pub struct LimitedView<'v, F: ReceiverFamily, L> {
    parent: ViewLoan<'v, F>,
    limit: &'v mut L,
}
impl<F: ReceiverFamily, L> fmt::Debug for LimitedView<'_, F, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LimitedView").finish_non_exhaustive()
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
            fn compose<'v, 'p>(
                self: Pin<&'v mut Self>, parent: Pin<&'v mut F::ReceiverView<'p>>,
            ) -> LimitedView<'v,F,Self>
            where F:'p, Self::Family:'v, 'p:'v {
                LimitedView {parent: ViewLoan::new(parent), limit:self.get_mut()}
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
    fn remaining_len<'v>(host: Pin<&mut Self::ReceiverView<'v>>) -> Self::Length
    where
        Self: 'v,
    {
        let view = host.get_mut();
        let parent = view
            .parent
            .with(|parent| F::remaining_len(parent.into_pin()));
        crate::Min::min(parent, view.limit.0.clone())
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
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'v,
    {
        let view = host.get_mut();
        let remaining = crate::Min::min(view.limit.0.clone(), usize::MAX);
        if remaining == 0 {
            return Poll::Ready(Ok(0));
        }
        let allowed = remaining.min(destination.remaining_mut());
        let count = ready!(view.parent.with(|parent| F::poll_read(
            parent.into_pin(),
            cx,
            &mut destination.limit(allowed)
        )))?;
        assert!(count <= allowed, "read exceeded the requested capacity");
        view.limit.consume(count);
        Poll::Ready(Ok(count))
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
    ) -> Poll<Result<Option<F::Chunk>, Self::Error>>
    where
        Self: 'v,
    {
        let view = host.get_mut();
        let Some(allowed) = NonZeroUsize::new(crate::Min::min(view.limit.0.clone(), maximum.get()))
        else {
            return Poll::Ready(Ok(None));
        };
        let chunk = ready!(view.parent.with(|parent| F::poll_read_chunk(
            parent.into_pin(),
            cx,
            allowed
        )))?;
        if let Some(chunk) = &chunk {
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
impl<F: Write> Write for Limited<F, WriteLimit> {
    type Error = F::Error;
    fn poll_write<'v, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'v,
    {
        let view = host.get_mut();
        let remaining = view.limit.0;
        // The boundary accepts nothing, without polling the parent.
        if remaining == 0 {
            return Poll::Ready(Ok(0));
        }
        let count = ready!(view.parent.with(|parent| {
            F::poll_write(parent.into_pin(), cx, &mut (&mut *input).take(remaining))
        }))?;
        view.limit.0 -= count;
        Poll::Ready(Ok(count))
    }
}

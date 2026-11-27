//! Byte budgets shared by reads, chunk reads, skips, and remainder queries.
use crate::{AtEnd, ChunkSegment, ExactLen, Min, Read, ReadChunk, ReadSegment, Skip};
use bytes::{Buf, BufMut, buf::Limit};
use core::{
    borrow::BorrowMut,
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    ops::SubAssign,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::{
    AccessPlan, Lent, ProvideAccess, Receiver, ReceiverAccess, ReceiverFamily, ViewProjection,
    project_access,
};

type ReadPoll<E, M> = Poll<Result<Option<ReadSegment<M>>, E>>;
type ChunkPoll<C, E, M> = Poll<Result<Option<ChunkSegment<C, M>>, E>>;

/// The inner IO and the remaining read quota returned by [`ReadLimit::into_parts`].
#[derive(Debug)]
pub struct ReadLimitParts<IO, B> {
    /// The wrapped IO.
    pub io: IO,
    /// The stored value or borrow of the remaining read quota.
    pub quota: B,
}

pin_project_lite::pin_project! {
    /// A source that lets reads and skips consume up to a byte quota.
    ///
    /// A zero quota answers EOF without polling the source. While the quota
    /// remains positive, even empty read requests reach the source. An
    /// [`AtEnd`] query answers EOF at zero and reaches the source otherwise.
    /// If the source implements [`ExactLen`] and its length can be compared
    /// with the quota, a length query reports their minimum.
    /// The quota type is independent of the size of an individual read. Use a
    /// typed literal such as `6u8` when the quota type is otherwise unknown.
    pub struct ReadLimit<IO, L = usize, B = L> {
        #[pin]
        io: IO,
        quota: B,
        length: PhantomData<fn() -> L>,
    }
}

impl<IO, L> ReadLimit<IO, L> {
    /// Wrap `io` with an owned quota, such as `6u8` or `6usize`.
    pub const fn new(io: IO, quota: L) -> Self {
        Self {
            io,
            quota,
            length: PhantomData,
        }
    }
}

impl<'q, IO, L> ReadLimit<IO, L, &'q mut L> {
    /// Wrap `io`, deducting progress from the caller's quota.
    pub const fn borrowing(io: IO, quota: &'q mut L) -> Self {
        Self {
            io,
            quota,
            length: PhantomData,
        }
    }
}

impl<IO, L, B> ReadLimit<IO, L, B> {
    /// Borrow the inner IO.
    pub const fn get_ref(&self) -> &IO {
        &self.io
    }

    /// Borrow the inner IO mutably.
    pub fn get_mut(&mut self) -> &mut IO {
        &mut self.io
    }

    /// Borrow the pinned inner IO.
    pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut IO> {
        self.project().io
    }

    /// Borrow the remaining read quota.
    pub const fn quota(&self) -> &B {
        &self.quota
    }

    /// Return the inner IO and the remaining read quota.
    pub fn into_parts(self) -> ReadLimitParts<IO, B> {
        ReadLimitParts {
            io: self.io,
            quota: self.quota,
        }
    }
}

impl<IO, L, B: fmt::Debug> fmt::Debug for ReadLimit<IO, L, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadLimit")
            .field("remaining", &self.quota)
            .finish_non_exhaustive()
    }
}

/// The family of a source of `F` bounded by a [`ReadLimit`] with quota `L`.
pub struct Limited<F, L>(PhantomData<fn(F) -> F>, PhantomData<L>);

impl<F: ReceiverFamily, L> ReceiverFamily for Limited<F, L> {
    type ReceiverView<'v>
        = ReadLimit<F::ReceiverView<'v>, L, &'v mut L>
    where
        Self: 'v;
}

struct LimitView<L>(PhantomData<L>);

impl<F: ReceiverFamily, L> ViewProjection<F> for LimitView<L> {
    type State = L;
    type Family = Limited<F, L>;

    fn project<'v>(
        view: F::ReceiverView<'v>,
        quota: &'v mut L,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v,
    {
        ReadLimit::borrowing(view, quota)
    }
}

impl<R: Receiver, L, B: BorrowMut<L>> Receiver for ReadLimit<R, L, B> {
    type Family = Limited<R::Family, L>;

    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        Self::Family: 'v,
    {
        let this = self.project();
        <LimitView<L> as ViewProjection<R::Family>>::project(
            this.io.view(),
            this.quota.borrow_mut(),
        )
    }
}

impl<A: ReceiverAccess, L, B: BorrowMut<L>> ProvideAccess for ReadLimit<A, L, B> {
    type AccessFamily = Limited<Lent<A::Family>, L>;
    type Lentness = <A as ProvideAccess>::Lentness;

    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        project_access::<_, LimitView<L>>(&mut self.io, self.quota.borrow_mut())
    }
}

impl<IO, L, B: BorrowMut<L>> ReadLimit<IO, L, B>
where
    L: Clone + Min<usize, Output = usize> + SubAssign<<usize as Min<L>>::Output>,
    usize: Min<L>,
{
    fn poll_read_with<O: BufMut + ?Sized, E, M>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        destination: &mut O,
        read: impl FnOnce(Pin<&mut IO>, &mut Context<'_>, &mut Limit<&mut O>) -> ReadPoll<E, M>,
    ) -> ReadPoll<E, M> {
        let view = self.project();
        let quota = view.quota.borrow_mut();
        let remaining = Min::min(quota.clone(), usize::MAX);
        if remaining == 0 {
            return Poll::Ready(Ok(None));
        }
        let allowed = Ord::min(remaining, destination.remaining_mut());
        let segment = ready!(read(view.io, cx, &mut destination.limit(allowed)))?;
        if let Some(segment) = &segment {
            let count = segment.count.get();
            assert!(count <= allowed, "read exceeded the requested capacity");
            *quota -= Min::min(count, quota.clone());
        }
        Poll::Ready(Ok(segment))
    }

    fn poll_read_chunk_with<C: Buf, E, M>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
        read: impl FnOnce(Pin<&mut IO>, &mut Context<'_>, NonZeroUsize) -> ChunkPoll<C, E, M>,
    ) -> ChunkPoll<C, E, M> {
        let view = self.project();
        let quota = view.quota.borrow_mut();
        let Some(allowed) = NonZeroUsize::new(Min::min(quota.clone(), maximum.get())) else {
            return Poll::Ready(Ok(None));
        };
        let chunk = ready!(read(view.io, cx, allowed))?;
        if let Some(ChunkSegment { chunk, .. }) = &chunk {
            let count = chunk.remaining();
            assert!(
                count > 0 && count <= allowed.get(),
                "invalid read chunk length"
            );
            *quota -= Min::min(count, quota.clone());
        }
        Poll::Ready(Ok(chunk))
    }
}

forward!(Read for [F: Read, L] Limited<F, L>, inner F, Metadata = F::Metadata, with poll_read_with,
    where [L: Clone + Min<usize, Output = usize> + SubAssign<<usize as Min<L>>::Output>, usize: Min<L>]);
forward!(ReadChunk for [F: ReadChunk, L] Limited<F, L>, inner F, with poll_read_chunk_with,
    where [L: Clone + Min<usize, Output = usize> + SubAssign<<usize as Min<L>>::Output>, usize: Min<L>]);

impl<F: AtEnd, L: Clone + Min<usize, Output = usize>> AtEnd for Limited<F, L> {
    type RemainderError = F::RemainderError;

    fn poll_at_end<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<bool, Self::RemainderError>>
    where
        Self: 'v,
    {
        let view = host.project();
        if Min::min((**view.quota).clone(), usize::MAX) == 0 {
            return Poll::Ready(Ok(true));
        }
        F::poll_at_end(view.io, cx)
    }
}

impl<F: ExactLen, L: Clone + Min<usize, Output = usize>> ExactLen for Limited<F, L>
where
    F::Length: Min<L>,
{
    type Length = <F::Length as Min<L>>::Output;

    fn poll_remaining_len<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Length, Self::RemainderError>>
    where
        Self: 'v,
    {
        let view = host.project();
        let parent = ready!(F::poll_remaining_len(view.io, cx))?;
        Poll::Ready(Ok(Min::min(parent, (**view.quota).clone())))
    }
}

/// The smaller of the caller's maximum and the quota, as one maximum.
#[derive(Clone)]
struct BothMaxima<M, L>(M, L);

impl<C, M: Min<C, Output = C>, L: Min<C, Output = C>> Min<C> for BothMaxima<M, L> {
    type Output = C;

    fn min(self, rhs: C) -> C {
        Min::min(self.0, Min::min(self.1, rhs))
    }
}

/// Skip up to the smaller of the caller's maximum and the read quota, using
/// the inner source's count type. At the quota boundary, return EOF without
/// polling the inner source.
///
/// # Panics
///
/// Panics if the inner source reports a count greater than the allowed maximum.
impl<F: Skip, L> Skip for Limited<F, L>
where
    L: Clone
        + Min<usize, Output = usize>
        + SubAssign<<usize as Min<L>>::Output>
        + Min<F::Count, Output = F::Count>
        + SubAssign<<F::Count as Min<L>>::Output>,
    usize: Min<L>,
    F::Count: Min<L> + Clone + PartialOrd,
{
    type Count = F::Count;

    fn poll_skip<'v, M: Min<F::Count, Output = F::Count> + Clone>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<F::Count>, Self::Error>>
    where
        Self: 'v,
    {
        let view = host.project();
        let quota = &mut **view.quota;
        let budget = quota.clone();
        if Min::min(budget.clone(), usize::MAX) == 0 {
            return Poll::Ready(Ok(None));
        }
        let allowed = BothMaxima(maximum, budget);
        let skipped = ready!(F::poll_skip(view.io, cx, allowed.clone()))?;
        if let Some(count) = &skipped {
            assert!(
                *count <= Min::min(allowed, count.clone()),
                "skip exceeded the requested maximum"
            );
            // The returned count fits both maxima. The minimum preserves
            // the count in the quota's subtraction type.
            *quota -= Min::min(count.clone(), quota.clone());
        }
        Poll::Ready(Ok(skipped))
    }
}

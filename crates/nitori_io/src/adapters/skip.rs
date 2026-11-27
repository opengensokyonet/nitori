//! Skipping by reading and discarding bytes or chunks.
use crate::{ExactLen, Read, ReadChunk, Skip, Write, WriteLent};
use bytes::Buf;
use core::{
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

// Match the bridge read buffer. Scratch storage lasts for only one poll.
const SCRATCH_LEN: usize = 8192;

/// The family of a wrapped view of `F`.
///
/// Use `ReadSkip<()>` for `S` to skip by reading or `ReadChunkSkip<()>`
/// to skip by dropping chunks. The IO parameter of the marker is `()`.
///
/// Both forward [`Read`], [`ReadChunk`], [`ExactLen`], [`Write`] and
/// [`WriteLent`] when `F` supports them.
pub struct Skippable<F, S>(PhantomData<fn(F) -> F>, PhantomData<S>);

macro_rules! skip_wrapper {
    ($(#[$doc:meta])* $W:ident, $proj:ident) => {
        pin_project_lite::pin_project! {
            $(#[$doc])*
            pub struct $W<IO> {
                #[pin]
                io: IO,
            }
        }
        impl<IO> $W<IO> {
            /// Wrap `io` to add skipping.
            pub const fn new(io: IO) -> Self {
                Self { io }
            }
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
            /// Return the inner IO.
            pub fn into_inner(self) -> IO {
                self.io
            }
        }
        impl<IO> fmt::Debug for $W<IO> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($W)).finish_non_exhaustive()
            }
        }
        impl<F: ReceiverFamily> ReceiverFamily for Skippable<F, $W<()>> {
            type ReceiverView<'v>
                = $W<F::ReceiverView<'v>>
            where
                Self: 'v;
        }
        struct $proj;
        impl<F: ReceiverFamily> ViewProjection<F> for $proj {
            type State = [(); 0];
            type Family = Skippable<F, $W<()>>;
            fn project<'v>(
                view: F::ReceiverView<'v>,
                _: &'v mut [(); 0],
            ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
            where
                F: 'v,
                Self::Family: 'v,
            {
                $W::new(view)
            }
        }
        impl<R: Receiver> Receiver for $W<R> {
            type Family = Skippable<R::Family, $W<()>>;
            fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
            where
                Self::Family: 'v,
            {
                <$proj as ViewProjection<R::Family>>::project(self.project().io.view(), &mut [])
            }
        }
        impl<A: ReceiverAccess> ProvideAccess for $W<A> {
            type AccessFamily = Skippable<Lent<A::Family>, $W<()>>;
            type Lentness = <A as ProvideAccess>::Lentness;
            fn access_plan<'a>(
                &'a mut self,
            ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
                project_access::<_, $proj>(&mut self.io, &mut [])
            }
        }
        forward!(Read for [F: Read] Skippable<F, $W<()>>, inner F, Metadata = <F as Read>::Metadata, pass);
        forward!(ReadChunk for [F: ReadChunk] Skippable<F, $W<()>>, inner F, pass);
        forward!(ExactLen for [F: ExactLen] Skippable<F, $W<()>>, inner F, pass);
        forward!(Write<O> for [F: Write<O>, O] Skippable<F, $W<()>>, inner F, options O, pass);
        forward!(WriteLent<O> for [F: WriteLent<O>, O] Skippable<F, $W<()>>, inner F, pass);
    };
}

skip_wrapper!(
    /// A source that skips by reading up to 8 KiB into a stack buffer on
    /// each poll and discarding the bytes. It keeps no additional state.
    /// Each poll reports what it consumed, so a count tracked by the
    /// caller stays in step across cancellation.
    ///
    /// The skipped count is `usize`. A maximum of zero polls an empty read.
    ReadSkip,
    ReadSkipView
);
skip_wrapper!(
    /// A source that skips by reading and dropping one chunk per poll.
    ///
    /// The skipped count is `usize`. A maximum above `usize::MAX` or
    /// [`Infinite`](crate::Infinite) requests a chunk of at most `usize::MAX`.
    /// A maximum of zero polls an empty read, which can still report the
    /// source's terminal error.
    ReadChunkSkip,
    ReadChunkSkipView
);

/// Reads into scratch storage and drops what was read.
impl<F: Read> Skip for Skippable<F, ReadSkip<()>> {
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
        let segment = ready!(F::poll_read(
            host.get_pin_mut(),
            cx,
            &mut &mut scratch[..allowed]
        ))?;
        let count = segment.map_or(0, |segment| segment.count.get());
        assert!(count <= allowed, "read exceeded the requested capacity");
        // An empty read cannot report EOF; only a nonempty one can.
        Poll::Ready(Ok((allowed == 0 || count != 0).then_some(count)))
    }
}

/// Takes one chunk and drops it.
impl<F: ReadChunk> Skip for Skippable<F, ReadChunkSkip<()>> {
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
            ready!(F::poll_read(host.get_pin_mut(), cx, &mut empty))?;
            return Poll::Ready(Ok(Some(0)));
        };
        let chunk = ready!(F::poll_read_chunk(host.get_pin_mut(), cx, allowed))?;
        Poll::Ready(Ok(chunk.map(|segment| {
            let count = segment.chunk.remaining();
            assert!(
                count > 0 && count <= allowed.get(),
                "invalid read chunk length"
            );
            count
        })))
    }
}

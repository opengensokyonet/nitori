//! A shared byte budget for ordinary and lent writes.
use crate::{LentOutcome, Write, WriteLent, WriteSegment};
use bytes::{Buf, buf::Take};
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

type WritePoll<R, E> = Poll<Result<Option<WriteSegment<R>>, E>>;
type LendPoll<R, T, A, E> = Poll<Result<Option<LentOutcome<R, T, A>>, E>>;

/// The inner IO and the remaining write budget returned by [`WriteLimit::into_parts`].
#[derive(Debug)]
pub struct WriteLimitParts<IO, B> {
    /// The wrapped IO.
    pub io: IO,
    /// The stored value or borrow of the remaining write budget.
    pub budget: B,
}

pin_project_lite::pin_project! {
    /// A sink that accepts up to a byte budget, shared by ordinary and lent writes.
    ///
    /// At zero it accepts no bytes without polling the sink. With input
    /// remaining, [`write_all`](crate::calls::write_all) reports this as zero
    /// progress. With budget remaining, requests reach the inner sink,
    /// including empty ones.
    /// The budget is spent only on accepted writes and persists between calls
    /// on a wrapped receiver. The wrapper exposes only [`Write`] and
    /// [`WriteLent`].
    pub struct WriteLimit<IO, B = usize> {
        #[pin]
        io: IO,
        budget: B,
    }
}

impl<IO> WriteLimit<IO> {
    /// Wrap `io` with an owned byte budget.
    pub const fn new(io: IO, budget: usize) -> Self {
        Self { io, budget }
    }
}

impl<'q, IO> WriteLimit<IO, &'q mut usize> {
    /// Wrap `io`, updating the caller's budget as writes succeed.
    pub const fn borrowing(io: IO, budget: &'q mut usize) -> Self {
        Self { io, budget }
    }
}

impl<IO, B> WriteLimit<IO, B> {
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

    /// Borrow the remaining write budget.
    pub const fn budget(&self) -> &B {
        &self.budget
    }

    /// Return the inner IO and the remaining write budget.
    pub fn into_parts(self) -> WriteLimitParts<IO, B> {
        WriteLimitParts {
            io: self.io,
            budget: self.budget,
        }
    }
}

impl<IO, B: fmt::Debug> fmt::Debug for WriteLimit<IO, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteLimit")
            .field("remaining", &self.budget)
            .finish_non_exhaustive()
    }
}

impl<IO, B: BorrowMut<usize>> WriteLimit<IO, B> {
    /// At the boundary it accepts nothing itself, without polling the io;
    /// within the budget the io is offered at most what is left.
    fn poll_write_with<I: Buf + ?Sized, O, R, E>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &mut I,
        options: &O,
        write: impl FnOnce(Pin<&mut IO>, &mut Context<'_>, &mut Take<&mut I>, &O) -> WritePoll<R, E>,
    ) -> WritePoll<R, E> {
        let this = self.project();
        let budget = this.budget.borrow_mut();
        let remaining = *budget;
        if remaining == 0 {
            return Poll::Ready(Ok(None));
        }
        let segment = ready!(write(
            this.io,
            cx,
            &mut (&mut *input).take(remaining),
            options
        ))?;
        if let Some(segment) = &segment {
            let count = segment.count.get();
            assert!(count <= remaining, "write exceeded the offered input");
            *budget -= count;
        }
        Poll::Ready(Ok(segment))
    }
    /// Lends at most the remaining budget of the io's storage.
    fn poll_write_lent_with<R, T, A, E>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        lend: impl FnOnce(Pin<&mut IO>, &mut Context<'_>, NonZeroUsize) -> LendPoll<R, T, A, E>,
    ) -> LendPoll<R, T, A, E> {
        let this = self.project();
        let budget = this.budget.borrow_mut();
        let Some(remaining) = NonZeroUsize::new(*budget) else {
            return Poll::Ready(Ok(None));
        };
        let allowed = wanted.min(remaining);
        let result = ready!(lend(this.io, cx, allowed))?;
        if let Some(LentOutcome::Kept(segment)) = &result {
            let count = segment.count.get();
            assert!(count <= allowed.get(), "write exceeded the granted count");
            *budget -= count;
        }
        Poll::Ready(Ok(result))
    }
}

/// The family of writes to `F` through a [`WriteLimit`].
pub struct WriteLimited<F>(PhantomData<fn(F) -> F>);

impl<F: ReceiverFamily> ReceiverFamily for WriteLimited<F> {
    type ReceiverView<'v>
        = WriteLimit<F::ReceiverView<'v>, &'v mut usize>
    where
        Self: 'v;
}

struct WriteLimitView;

impl<F: ReceiverFamily> ViewProjection<F> for WriteLimitView {
    type State = usize;
    type Family = WriteLimited<F>;
    fn project<'v>(
        view: F::ReceiverView<'v>,
        state: &'v mut usize,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v,
    {
        WriteLimit::borrowing(view, state)
    }
}

impl<R: Receiver, B: BorrowMut<usize>> Receiver for WriteLimit<R, B> {
    type Family = WriteLimited<R::Family>;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        Self::Family: 'v,
    {
        let this = self.project();
        <WriteLimitView as ViewProjection<R::Family>>::project(
            this.io.view(),
            this.budget.borrow_mut(),
        )
    }
}

impl<A: ReceiverAccess, B: BorrowMut<usize>> ProvideAccess for WriteLimit<A, B> {
    type AccessFamily = WriteLimited<Lent<A::Family>>;
    type Lentness = <A as ProvideAccess>::Lentness;
    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        project_access::<_, WriteLimitView>(&mut self.io, self.budget.borrow_mut())
    }
}

forward!(Write<O> for [F: Write<O>, O] WriteLimited<F>, inner F, options O, with poll_write_with);
forward!(WriteLent<O> for [F: WriteLent<O>, O] WriteLimited<F>, inner F, with poll_write_lent_with);

use bytes::BufMut;
use nitori_call::{Compose, HasReceiverFamily, ReceiverFamily, ViewLoan};
use nitori_io::Read;
use std::{
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll, ready},
};

/// A byte budget owned by the operation, retained across polls.
pub struct ReadLimit(pub usize);

pub struct Limited<F>(PhantomData<fn(F) -> F>);
pub struct LimitedView<'v, F: ReceiverFamily> {
    inner: ViewLoan<'v, F>,
    remaining: &'v mut usize,
}
impl<F: ReceiverFamily> ReceiverFamily for Limited<F> {
    type ReceiverView<'v>
        = LimitedView<'v, F>
    where
        Self: 'v;
}
impl<F: ReceiverFamily> HasReceiverFamily for LimitedView<'_, F> {
    type Family = Limited<F>;
}
impl<F: ReceiverFamily> Compose<F> for ReadLimit {
    type Family = Limited<F>;
    fn compose<'v, 'p>(
        self: Pin<&'v mut Self>,
        inner: Pin<&'v mut F::ReceiverView<'p>>,
    ) -> LimitedView<'v, F>
    where
        F: 'p,
        Self::Family: 'v,
        'p: 'v,
    {
        LimitedView {
            inner: ViewLoan::new(inner),
            remaining: &mut self.get_mut().0,
        }
    }
}
impl<F: Read> Read for Limited<F> {
    type Error = F::Error;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'v,
    {
        let view = host.get_mut();
        // Reaching the record boundary is EOF for this view, without polling the parent.
        if *view.remaining == 0 {
            return Poll::Ready(Ok(0));
        }
        let count = ready!(view.inner.with(|access| {
            F::poll_read(access.into_pin(), cx, &mut out.limit(*view.remaining))
        }))?;
        *view.remaining -= count;
        Poll::Ready(Ok(count))
    }
}

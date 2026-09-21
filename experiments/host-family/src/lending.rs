//! Explicit reconstruction of temporary views; no variance assumption.
use crate::{Call, Family, Read, ReadFamily};
use std::{
    convert::Infallible,
    marker::PhantomData,
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll},
};

/// Reconstruct a short view of the same logical resource. Persistent state must
/// remain in its owner; rebuilding the view must not reset or duplicate it.
pub trait Reborrow: Family {
    fn reborrow<'short, 'host>(host: Pin<&'short mut Self::Host<'host>>) -> Self::Host<'short>
    where
        Self: 'host,
        'host: 'short;
}

/// A sized access handle for a possibly unsized, pinned underlying host.
pub struct Root<H: ?Sized>(PhantomData<fn() -> H>);
impl<H: ?Sized> Family for Root<H> {
    type Host<'host>
        = Pin<&'host mut H>
    where
        Self: 'host;
}
impl<H: ?Sized> Reborrow for Root<H> {
    fn reborrow<'short, 'host>(host: Pin<&'short mut Self::Host<'host>>) -> Self::Host<'short>
    where
        Self: 'host,
        'host: 'short,
    {
        host.get_mut().as_mut()
    }
}
impl<H: Read + ?Sized> ReadFamily for Root<H> {
    type Error = H::Error;
    fn read<'host>(
        host: Pin<&mut Self::Host<'host>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<u8, H::Error>>
    where
        Self: 'host,
    {
        host.get_mut().as_mut().read(cx)
    }
}

/// The author supplies the view type, construction, and reconstruction.
/// The inner value is already a temporary access view, not the underlying host.
pub trait Layer<F: Reborrow> {
    type Host<'host>
    where
        Self: 'host,
        F: 'host;
    fn compose<'host>(self: Pin<&'host mut Self>, inner: F::Host<'host>) -> Self::Host<'host>
    where
        F: 'host;
    fn reborrow<'short, 'host>(host: Pin<&'short mut Self::Host<'host>>) -> Self::Host<'short>
    where
        Self: 'host,
        F: 'host,
        'host: 'short;
}

pub struct Layered<F, S>(PhantomData<fn() -> (F, S)>);
impl<F: Reborrow, S: Layer<F>> Family for Layered<F, S> {
    type Host<'host>
        = S::Host<'host>
    where
        Self: 'host;
}
impl<F: Reborrow, S: Layer<F>> Reborrow for Layered<F, S> {
    fn reborrow<'short, 'host>(host: Pin<&'short mut Self::Host<'host>>) -> Self::Host<'short>
    where
        Self: 'host,
        'host: 'short,
    {
        S::reborrow(host)
    }
}

pub trait ReadLayer<F: Reborrow>: Layer<F> {
    type Error;
    fn read<'host>(
        host: Pin<&mut Self::Host<'host>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<u8, Self::Error>>
    where
        Self: 'host,
        F: 'host;
}
impl<F: Reborrow, S: ReadLayer<F>> ReadFamily for Layered<F, S> {
    type Error = S::Error;
    fn read<'host>(
        host: Pin<&mut Self::Host<'host>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<u8, Self::Error>>
    where
        Self: 'host,
    {
        S::read(host, cx)
    }
}

pub struct Composed<'state, F: Reborrow, S: Layer<F>, O> {
    state: Pin<&'state mut S>,
    operation: O,
    marker: PhantomData<fn() -> F>,
}
impl<'state, F: Reborrow, S: Layer<F>, O> Composed<'state, F, S, O> {
    pub fn new(state: Pin<&'state mut S>, operation: O) -> Self {
        Self {
            state,
            operation,
            marker: PhantomData,
        }
    }
}
impl<F: Reborrow, S: Layer<F>, O: Call<Layered<F, S>>> Call<F> for Composed<'_, F, S, O> {
    type Yield = O::Yield;
    type Return = O::Return;
    fn poll<'host>(
        self: Pin<&mut Self>,
        host: Pin<&mut F::Host<'host>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        F: 'host,
    {
        // SAFETY: operation is structurally pinned and never moved. state is a
        // movable pinned reference, not its pinned referent.
        let this = unsafe { self.get_unchecked_mut() };
        let inner = F::reborrow(host);
        let mut view = std::pin::pin!(this.state.as_mut().compose(inner));
        unsafe { Pin::new_unchecked(&mut this.operation) }.poll(view.as_mut(), cx)
    }
}

/// A nominal argument hides the GAT projection from the higher-ranked Fn bound.
/// Both lifetimes stay independent; no reference is extended or reinterpreted.
pub struct Access<'access, 'host, F: Family + 'host> {
    host: Pin<&'access mut F::Host<'host>>,
}
impl<'access, 'host, F: Family + 'host> Access<'access, 'host, F> {
    pub fn into_pin(self) -> Pin<&'access mut F::Host<'host>> {
        self.host
    }
}

pub struct With<F, B, R> {
    body: Option<B>,
    marker: PhantomData<fn() -> (F, R)>,
}
// No field is structurally pinned or ever exposed through a pinned reference.
impl<F, B, R> Unpin for With<F, B, R> {}

pub fn with<F: Family, B, R>(body: B) -> With<F, B, R>
where
    B: for<'access, 'host> FnOnce(Access<'access, 'host, F>) -> R,
{
    With {
        body: Some(body),
        marker: PhantomData,
    }
}
impl<F: Family, B, R> Call<F> for With<F, B, R>
where
    B: for<'access, 'host> FnOnce(Access<'access, 'host, F>) -> R,
{
    type Yield = Infallible;
    type Return = R;
    fn poll<'host>(
        self: Pin<&mut Self>,
        host: Pin<&mut F::Host<'host>>,
        _: &mut Context<'_>,
    ) -> Poll<CoroutineState<Infallible, R>>
    where
        F: 'host,
    {
        let body = self
            .get_mut()
            .body
            .take()
            .expect("with polled after completion or panic");
        Poll::Ready(CoroutineState::Complete(body(Access { host })))
    }
}

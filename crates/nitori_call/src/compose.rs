//! Real receiver values and author-defined composition.
use crate::{CallOn, Child, Host, HostFamily};
use std::{
    convert::Infallible,
    marker::PhantomData,
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll},
};

/// Host-free access to the current call's root family.
pub struct Receiver<F: HostFamily>(PhantomData<fn() -> F>);
impl<F: HostFamily> Copy for Receiver<F> {}
impl<F: HostFamily> Clone for Receiver<F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F: HostFamily> Default for Receiver<F> {
    fn default() -> Self {
        Self::new()
    }
}
impl<F: HostFamily> Receiver<F> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

/// A recipe for selecting the host of a child operation.
pub trait Route {
    type Root: HostFamily;
    type Target: HostFamily;
    fn view<'access, 'host>(
        self: Pin<&'access mut Self>,
        host: Pin<&'access mut <Self::Root as HostFamily>::Host<'host>>,
    ) -> <Self::Target as HostFamily>::Host<'access>
    where
        Self::Root: 'host,
        Self::Target: 'access,
        'host: 'access;
}
impl<F: HostFamily> Route for Receiver<F> {
    type Root = F;
    type Target = F;
    fn view<'a, 'h>(self: Pin<&'a mut Self>, host: Pin<&'a mut F::Host<'h>>) -> F::Host<'a>
    where
        F: 'h,
        'h: 'a,
    {
        host.view()
    }
}
/// Compose a fresh inner view with state. The associated family owns the view design.
pub trait Compose<F: HostFamily> {
    type Family: HostFamily;
    fn compose<'a>(
        self: Pin<&'a mut Self>,
        inner: F::Host<'a>,
    ) -> <Self::Family as HostFamily>::Host<'a>
    where
        F: 'a,
        Self::Family: 'a;
}
impl<F: HostFamily, S: Compose<F> + Unpin + ?Sized> Compose<F> for &mut S {
    type Family = S::Family;
    fn compose<'a>(
        self: Pin<&'a mut Self>,
        inner: F::Host<'a>,
    ) -> <Self::Family as HostFamily>::Host<'a>
    where
        F: 'a,
        Self::Family: 'a,
    {
        Pin::new(&mut **self.get_mut()).compose(inner)
    }
}
impl<F: HostFamily, S: Compose<F> + ?Sized> Compose<F> for Pin<&mut S> {
    type Family = S::Family;
    fn compose<'a>(
        self: Pin<&'a mut Self>,
        inner: F::Host<'a>,
    ) -> <Self::Family as HostFamily>::Host<'a>
    where
        F: 'a,
        Self::Family: 'a,
    {
        self.get_mut().as_mut().compose(inner)
    }
}
pin_project_lite::pin_project! {
    pub struct Composed<R, S> { #[pin] route: R, #[pin] state: S }
}
impl<R: Route, S: Compose<R::Target>> Route for Composed<R, S> {
    type Root = R::Root;
    type Target = S::Family;
    fn view<'a, 'h>(
        self: Pin<&'a mut Self>,
        host: Pin<&'a mut <R::Root as HostFamily>::Host<'h>>,
    ) -> <S::Family as HostFamily>::Host<'a>
    where
        R::Root: 'h,
        S::Family: 'a,
        'h: 'a,
    {
        let this = self.project();
        this.state.compose(this.route.view(host))
    }
}
impl<R: Route + Unpin + ?Sized> Route for &mut R {
    type Root = R::Root;
    type Target = R::Target;
    fn view<'a, 'h>(
        self: Pin<&'a mut Self>,
        host: Pin<&'a mut <R::Root as HostFamily>::Host<'h>>,
    ) -> <R::Target as HostFamily>::Host<'a>
    where
        R::Root: 'h,
        R::Target: 'a,
        'h: 'a,
    {
        Pin::new(&mut **self.get_mut()).view(host)
    }
}
impl<R: Route + ?Sized> Route for Pin<&mut R> {
    type Root = R::Root;
    type Target = R::Target;
    fn view<'a, 'h>(
        self: Pin<&'a mut Self>,
        host: Pin<&'a mut <R::Root as HostFamily>::Host<'h>>,
    ) -> <R::Target as HostFamily>::Host<'a>
    where
        R::Root: 'h,
        R::Target: 'a,
        'h: 'a,
    {
        self.get_mut().as_mut().view(host)
    }
}
/// Construct an operation for a selected family.
pub trait Arguments<F: HostFamily> {
    type Call: CallOn<F>;
    fn into_call(self) -> Self::Call;
}

pub trait ReceiverExt: Route + Sized {
    fn compose<S: Compose<Self::Target>>(self, state: S) -> Composed<Self, S> {
        Composed { route: self, state }
    }
    fn call<A: Arguments<Self::Target>>(
        self,
        arguments: A,
    ) -> Child<Self::Root, Routed<Self, A::Call>> {
        self.operation(arguments.into_call())
    }
    fn operation<O: CallOn<Self::Target>>(
        self,
        operation: O,
    ) -> Child<Self::Root, Routed<Self, O>> {
        Child::new(Routed {
            route: self,
            operation,
        })
    }
    fn with<B, R>(self, body: B) -> WithChild<Self, B, R>
    where
        B: for<'a, 'h> FnOnce(Access<'a, 'h, Self::Target>) -> R,
    {
        self.operation(With {
            body: Some(body),
            marker: PhantomData,
        })
    }
}
impl<R: Route> ReceiverExt for R {}
pin_project_lite::pin_project! {
    pub struct Routed<R,O> { #[pin] route:R, #[pin] operation:O }
}
impl<R: Route, O: CallOn<R::Target>> CallOn<R::Root> for Routed<R, O> {
    type Yield = O::Yield;
    type Return = O::Return;
    fn poll_call<'h>(
        self: Pin<&mut Self>,
        host: Pin<&mut <R::Root as HostFamily>::Host<'h>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        R::Root: 'h,
    {
        let this = self.project();
        let mut view = std::pin::pin!(this.route.view(host));
        this.operation.poll_call(view.as_mut(), cx)
    }
}
/// A nominal lifetime boundary for one synchronous access callback.
pub struct Access<'a, 'h, F: HostFamily + 'h> {
    host: Pin<&'a mut F::Host<'h>>,
}
impl<'a, 'h, F: HostFamily + 'h> Access<'a, 'h, F> {
    pub fn into_pin(self) -> Pin<&'a mut F::Host<'h>> {
        self.host
    }
}
pub struct With<F: HostFamily, B, R> {
    body: Option<B>,
    marker: PhantomData<fn() -> (F, R)>,
}
impl<F: HostFamily, B, R> Unpin for With<F, B, R> {}
impl<F: HostFamily, B, R> CallOn<F> for With<F, B, R>
where
    B: for<'a, 'h> FnOnce(Access<'a, 'h, F>) -> R,
{
    type Yield = Infallible;
    type Return = R;
    fn poll_call<'h>(
        self: Pin<&mut Self>,
        host: Pin<&mut F::Host<'h>>,
        _: &mut Context<'_>,
    ) -> Poll<CoroutineState<Infallible, R>>
    where
        F: 'h,
    {
        Poll::Ready(CoroutineState::Complete(self
            .get_mut()
            .body
            .take()
            .expect("with polled after completion")(
            Access { host }
        )))
    }
}
impl<R, S> Composed<R, S> {
    /// Recover the description and state once no child borrows this value.
    pub fn into_parts(self) -> (R, S) {
        (self.route, self.state)
    }
}

/// A routed synchronous host-access request.
pub type WithChild<R, B, Output> =
    Child<<R as Route>::Root, Routed<R, With<<R as Route>::Target, B, Output>>>;

#![allow(clippy::multiple_bound_locations)] // pin-project-lite projection bounds
//! Typed target descriptions and lazy, scoped composition.
use crate::{CallOn, Child, ReceiverFamily, ReceiverScope};
use std::{
    convert::Infallible,
    marker::PhantomData,
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll, ready},
};
pub struct Target<F: ReceiverFamily>(PhantomData<fn() -> F>);
impl<F: ReceiverFamily> Copy for Target<F> {}
impl<F: ReceiverFamily> Clone for Target<F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F: ReceiverFamily> Default for Target<F> {
    fn default() -> Self {
        Self::new()
    }
}
impl<F: ReceiverFamily> Target<F> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

/// One projection; the parent view keeps its original inner lifetime.
pub trait Compose<F: ReceiverFamily> {
    type Family: ReceiverFamily;
    fn compose<'scope, 'parent>(
        self: Pin<&'scope mut Self>,
        parent: Pin<&'scope mut F::ReceiverView<'parent>>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'scope>
    where
        F: 'parent,
        Self::Family: 'scope,
        'parent: 'scope;
}
impl<F: ReceiverFamily, S: Compose<F> + Unpin + ?Sized> Compose<F> for &mut S {
    type Family = S::Family;
    fn compose<'s, 'p>(
        self: Pin<&'s mut Self>,
        parent: Pin<&'s mut F::ReceiverView<'p>>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'s>
    where
        F: 'p,
        Self::Family: 's,
        'p: 's,
    {
        Pin::new(&mut **self.get_mut()).compose(parent)
    }
}
impl<F: ReceiverFamily, S: Compose<F> + ?Sized> Compose<F> for Pin<&mut S> {
    type Family = S::Family;
    fn compose<'s, 'p>(
        self: Pin<&'s mut Self>,
        parent: Pin<&'s mut F::ReceiverView<'p>>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'s>
    where
        F: 'p,
        Self::Family: 's,
        'p: 's,
    {
        self.get_mut().as_mut().compose(parent)
    }
}
pub trait CallTarget {
    type Root: ReceiverFamily;
    type Target: ReceiverFamily;
    fn poll_call<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<O::Yield, O::Return>>
    where
        Self::Root: 'v;
    fn poll_return<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<O::Return>
    where
        Self::Root: 'v;
}
impl<F: ReceiverFamily> CallTarget for Target<F> {
    type Root = F;
    type Target = F;
    fn poll_call<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<O::Yield, O::Return>>
    where
        Self::Root: 'v,
    {
        operation.poll_call(scope, cx)
    }
    fn poll_return<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<O::Return>
    where
        Self::Root: 'v,
    {
        operation.poll_return(scope, cx)
    }
}
impl<R: CallTarget + Unpin + ?Sized> CallTarget for &mut R {
    type Root = R::Root;
    type Target = R::Target;
    fn poll_call<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<O::Yield, O::Return>>
    where
        Self::Root: 'v,
    {
        Pin::new(&mut **self.get_mut()).poll_call(scope, operation, cx)
    }
    fn poll_return<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<O::Return>
    where
        Self::Root: 'v,
    {
        Pin::new(&mut **self.get_mut()).poll_return(scope, operation, cx)
    }
}
impl<R: CallTarget + ?Sized> CallTarget for Pin<&mut R> {
    type Root = R::Root;
    type Target = R::Target;
    fn poll_call<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<O::Yield, O::Return>>
    where
        Self::Root: 'v,
    {
        self.get_mut().as_mut().poll_call(scope, operation, cx)
    }
    fn poll_return<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<O::Return>
    where
        Self::Root: 'v,
    {
        self.get_mut().as_mut().poll_return(scope, operation, cx)
    }
}
impl<R: CallTarget, S: Compose<R::Target>> CallTarget for Composed<R, S> {
    type Root = R::Root;
    type Target = S::Family;
    fn poll_call<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<O::Yield, O::Return>>
    where
        Self::Root: 'v,
    {
        {
            let this = self.project();
            let layer = LayerCall::<R::Target, S, O> {
                state: this.state,
                operation,
                family: PhantomData,
            };
            this.receiver.poll_call(scope, pin!(layer), cx)
        }
    }
    fn poll_return<'v, O: CallOn<Self::Target> + ?Sized>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = Self::Root>>,
        operation: Pin<&mut O>,
        cx: &mut Context<'_>,
    ) -> Poll<O::Return>
    where
        Self::Root: 'v,
    {
        {
            let this = self.project();
            let layer = LayerCall::<R::Target, S, O> {
                state: this.state,
                operation,
                family: PhantomData,
            };
            this.receiver.poll_return(scope, pin!(layer), cx)
        }
    }
}
pin_project_lite::pin_project! {
 pub struct Composed<R,S> { #[pin] receiver:R, #[pin] state:S }
}
impl<R, S> Composed<R, S> {
    pub fn into_parts(self) -> (R, S) {
        (self.receiver, self.state)
    }
}
pin_project_lite::pin_project! {
 struct ComposeScope<'s,'p:'s,F:ReceiverFamily,S> where F:'p, S:Compose<F>, S::Family:'s {
  parent:Option<Pin<&'s mut dyn ReceiverScope<'p,Family=F>>>,
  state:Option<Pin<&'s mut S>>,
  #[pin] view:Option<<S::Family as ReceiverFamily>::ReceiverView<'s>>,
 }
}
impl<'s, 'p, F: ReceiverFamily + 'p, S: Compose<F>> ReceiverScope<'s> for ComposeScope<'s, 'p, F, S>
where
    S::Family: 's,
    'p: 's,
{
    type Family = S::Family;
    fn poll_view(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Pin<&mut <S::Family as ReceiverFamily>::ReceiverView<'s>>> {
        let mut this = self.project();
        if this.view.is_none() {
            ready!(
                this.parent
                    .as_mut()
                    .expect("composition initialization panicked")
                    .as_mut()
                    .poll_ready(cx)
            );
            let Poll::Ready(parent) = this.parent.take().unwrap().poll_view(cx) else {
                panic!("scope violated sticky readiness")
            };
            this.view
                .set(Some(this.state.take().unwrap().compose(parent)));
        }
        Poll::Ready(this.view.as_pin_mut().unwrap())
    }
}
struct LayerCall<'a, F, S, O: ?Sized> {
    state: Pin<&'a mut S>,
    operation: Pin<&'a mut O>,
    family: PhantomData<fn(F) -> F>,
}
impl<F, S, O: ?Sized> Unpin for LayerCall<'_, F, S, O> {}
impl<F: ReceiverFamily, S: Compose<F>, O: CallOn<S::Family> + ?Sized> CallOn<F>
    for LayerCall<'_, F, S, O>
{
    type Yield = O::Yield;
    type Return = O::Return;
    fn poll_call<'v>(
        self: Pin<&mut Self>,
        parent: Pin<&mut dyn ReceiverScope<'v, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        F: 'v,
    {
        let this = self.get_mut();
        let mut scope = pin!(ComposeScope {
            parent: Some(parent),
            state: Some(this.state.as_mut()),
            view: None
        });
        this.operation.as_mut().poll_call(scope.as_mut(), cx)
    }
    fn poll_return<'v>(
        self: Pin<&mut Self>,
        parent: Pin<&mut dyn ReceiverScope<'v, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Return>
    where
        F: 'v,
    {
        let this = self.get_mut();
        let mut scope = pin!(ComposeScope {
            parent: Some(parent),
            state: Some(this.state.as_mut()),
            view: None
        });
        this.operation.as_mut().poll_return(scope.as_mut(), cx)
    }
}
/// Construct an operation for a selected family.
pub trait Arguments<F: ReceiverFamily> {
    type Call: CallOn<F>;
    fn into_call(self) -> Self::Call;
}

pub trait TargetExt: CallTarget + Sized {
    fn compose<S: Compose<Self::Target>>(self, state: S) -> Composed<Self, S> {
        Composed {
            receiver: self,
            state,
        }
    }
    fn call<A: Arguments<Self::Target>>(
        self,
        arguments: A,
    ) -> Child<Self::Root, AdaptedCall<Self, A::Call>> {
        self.operation(arguments.into_call())
    }
    fn operation<O: CallOn<Self::Target>>(
        self,
        operation: O,
    ) -> Child<Self::Root, AdaptedCall<Self, O>> {
        Child::new(AdaptedCall {
            receiver: self,
            operation,
            terminal: false,
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
impl<R: CallTarget> TargetExt for R {}
pin_project_lite::pin_project! {
 pub struct AdaptedCall<R,O> { #[pin] receiver:R, #[pin] operation:O, terminal:bool }
}
impl<R: CallTarget, O: CallOn<R::Target>> CallOn<R::Root> for AdaptedCall<R, O> {
    type Yield = O::Yield;
    type Return = O::Return;
    fn poll_call<'v>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = R::Root>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        R::Root: 'v,
    {
        let this = self.project();
        assert!(
            !*this.terminal,
            "adapted call polled after completion or panic"
        );
        *this.terminal = true;
        let result = this.receiver.poll_call(scope, this.operation, cx);
        if !matches!(&result, Poll::Ready(CoroutineState::Complete(_))) {
            *this.terminal = false;
        }
        result
    }
    fn poll_return<'v>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'v, Family = R::Root>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Return>
    where
        R::Root: 'v,
    {
        let this = self.project();
        assert!(
            !*this.terminal,
            "adapted call polled after completion or panic"
        );
        *this.terminal = true;
        let result = this.receiver.poll_return(scope, this.operation, cx);
        if result.is_pending() {
            *this.terminal = false;
        }
        result
    }
}
/// A nominal lifetime boundary for one synchronous access callback.
pub struct Access<'a, 'h, F: ReceiverFamily + 'h> {
    host: Pin<&'a mut F::ReceiverView<'h>>,
}
impl<'a, 'h, F: ReceiverFamily + 'h> Access<'a, 'h, F> {
    pub(crate) fn new(host: Pin<&'a mut F::ReceiverView<'h>>) -> Self {
        Self { host }
    }
    pub fn into_pin(self) -> Pin<&'a mut F::ReceiverView<'h>> {
        self.host
    }
}
pub struct With<F: ReceiverFamily, B, R> {
    body: Option<B>,
    marker: PhantomData<fn() -> (F, R)>,
}
impl<F: ReceiverFamily, B, R> Unpin for With<F, B, R> {}
impl<F: ReceiverFamily, B, R> CallOn<F> for With<F, B, R>
where
    B: for<'a, 'h> FnOnce(Access<'a, 'h, F>) -> R,
{
    type Yield = Infallible;
    type Return = R;
    fn poll_call<'h>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'h, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Infallible, R>>
    where
        F: 'h,
    {
        let host = ready!(scope.poll_view(cx));
        Poll::Ready(CoroutineState::Complete(self
            .get_mut()
            .body
            .take()
            .expect("with polled after completion")(
            Access { host }
        )))
    }
}
/// A synchronous host-access request bound to a call receiver.
pub type WithChild<R, B, Output> =
    Child<<R as CallTarget>::Root, AdaptedCall<R, With<<R as CallTarget>::Target, B, Output>>>;

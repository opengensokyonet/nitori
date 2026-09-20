// expect: E0277
#![feature(coroutines, coroutine_trait, closure_lifetime_binder)]
use std::{ops::Coroutine, pin::Pin, task::Context};
struct Visit<'target, 'context, 'waker, T: ?Sized> {
    target: Pin<&'target mut T>,
    cx: &'context mut Context<'waker>,
}
fn consume<T: ?Sized>(mut visit: Visit<'_, '_, '_, T>) {
    let _ = visit.target.as_mut();
    visit.cx.waker().wake_by_ref();
}
fn make<T>() -> impl for<'target, 'context, 'waker> Coroutine<
    Visit<'target, 'context, 'waker, T>,
    Yield = (),
    Return = (),
> {
    #[coroutine]
    for<'target, 'context, 'waker> static |visit: Visit<'target, 'context, 'waker, T>| -> () {
        consume(visit);
        let next = yield ();
        consume(next);
    }
}
fn main() {
    let _ = make::<usize>();
}

// expect: E0277
#![feature(coroutines, coroutine_trait, closure_lifetime_binder)]
use std::{ops::Coroutine, task::Context};
fn make() -> impl for<'visit, 'waker> Coroutine<&'visit mut Context<'waker>, Yield = (), Return = ()>
{
    #[coroutine]
    for<'visit, 'waker> |cx: &'visit mut Context<'waker>| -> () {
        cx.waker().wake_by_ref();
        let next = yield ();
        let _: &mut Context<'_> = next;
    }
}
fn main() {
    let _ = make();
}

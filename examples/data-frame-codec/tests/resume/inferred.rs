// expect: E0277
#![feature(coroutines, coroutine_trait, closure_lifetime_binder)]
use std::ops::Coroutine;
fn consume<T>(_: &mut T) {}
fn make<T>() -> impl for<'visit> Coroutine<&'visit mut T, Yield = (), Return = ()> {
    #[coroutine]
    |target: &mut T| {
        consume(target);
        let next = yield ();
        consume(next);
    }
}
fn main() {
    let _ = make::<usize>();
}

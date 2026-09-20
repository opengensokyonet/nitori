// expect: E0277
#![feature(coroutines, coroutine_trait, closure_lifetime_binder)]
use std::ops::Coroutine;
fn consume<T>(_: &mut T) {}
fn make<T>() -> impl for<'visit> Coroutine<&'visit mut T, Yield = (), Return = ()> {
    #[coroutine]
    for<'visit> |target: &'visit mut T| -> () {
        consume(target);
    }
}
fn main() {
    let _ = make::<usize>();
}

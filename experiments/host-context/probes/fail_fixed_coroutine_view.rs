#![feature(coroutines, coroutine_trait, stmt_expr_attributes)]
use std::{marker::PhantomData, ops::Coroutine, pin::Pin};

struct Layer<'a, V: ?Sized> {
    inner: Pin<&'a mut V>,
}
struct ResumeEnv<V: ?Sized>(PhantomData<fn(*mut V) -> *mut V>);

fn require_every_layer<V, S>(_: &S)
where
    for<'a> S: Coroutine<ResumeEnv<Layer<'a, V>>, Yield = (), Return = ()>,
{
}

fn main() {
    let state = #[coroutine]
    |_: ResumeEnv<Layer<'_, u8>>| {
        yield ();
    };
    require_every_layer::<u8, _>(&state);
}

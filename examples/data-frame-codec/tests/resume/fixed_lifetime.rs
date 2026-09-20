// expect: pass
#![feature(coroutines, coroutine_trait)]
use std::{
    ops::{Coroutine, CoroutineState},
    pin::pin,
};
fn make<'visit>() -> impl Coroutine<&'visit mut usize, Yield = (), Return = ()> {
    #[coroutine]
    |mut target: &'visit mut usize| {
        *target += 1;
        target = yield ();
        *target += 2;
    }
}
fn main() {
    let mut target = 0;
    let mut other = 0;
    {
        let mut state = pin!(make());
        assert!(matches!(
            state.as_mut().resume(&mut target),
            CoroutineState::Yielded(())
        ));
        assert!(matches!(
            state.as_mut().resume(&mut other),
            CoroutineState::Complete(())
        ));
    }
    assert_eq!(target, 1);
}

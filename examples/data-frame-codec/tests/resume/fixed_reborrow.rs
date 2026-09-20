// expect: E0499
#![feature(coroutines, coroutine_trait)]
use std::{ops::Coroutine, pin::pin};
fn main() {
    let mut target = 0;
    let mut state = pin!(
        #[coroutine]
        |mut target: &mut usize| {
            *target += 1;
            target = yield ();
            *target += 2;
        }
    );
    state.as_mut().resume(&mut target);
    state.as_mut().resume(&mut target);
}

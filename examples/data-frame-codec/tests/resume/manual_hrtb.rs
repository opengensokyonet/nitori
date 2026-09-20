// expect: pass
#![feature(coroutine_trait)]
#![forbid(unsafe_code)]
use std::{
    ops::{Coroutine, CoroutineState},
    pin::Pin,
    task::{Context, Waker},
};
struct State(bool);
impl<'target, 'context, 'waker> Coroutine<(Pin<&'target mut usize>, &'context mut Context<'waker>)>
    for State
{
    type Yield = ();
    type Return = ();
    fn resume(
        mut self: Pin<&mut Self>,
        (mut target, cx): (Pin<&'target mut usize>, &'context mut Context<'waker>),
    ) -> CoroutineState<(), ()> {
        *target += 1;
        cx.waker().wake_by_ref();
        if self.0 {
            CoroutineState::Complete(())
        } else {
            self.0 = true;
            CoroutineState::Yielded(())
        }
    }
}
fn require_hrtb(
    _: &impl for<'target, 'context, 'waker> Coroutine<
        (Pin<&'target mut usize>, &'context mut Context<'waker>),
        Yield = (),
        Return = (),
    >,
) {
}
fn main() {
    let mut state = State(false);
    require_hrtb(&state);
    let mut target = 0;
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        Pin::new(&mut state).resume((Pin::new(&mut target), &mut cx)),
        CoroutineState::Yielded(())
    ));
    target += 10;
    assert!(matches!(
        Pin::new(&mut state).resume((Pin::new(&mut target), &mut cx)),
        CoroutineState::Complete(())
    ));
    assert_eq!(target, 12);
}

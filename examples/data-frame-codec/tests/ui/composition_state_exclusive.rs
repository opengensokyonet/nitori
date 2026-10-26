// expect: E0506
// facade-only
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{Compose, Direct, DirectView, ReceiverAccess};
use std::{future::poll_fn, pin::Pin, task::Poll};
struct State(usize);
impl Compose<Direct<()>> for State {
    type Family = Direct<usize>;
    fn compose<'v, 'p>(self: Pin<&'v mut Self>, _: Pin<&'v mut DirectView<'p, ()>>) -> DirectView<'v, usize>
    where Direct<()>: 'p, Self::Family: 'v, 'p: 'v {
        DirectView(Pin::new(&mut self.get_mut().0))
    }
}
// The state cannot change while the adapted access borrows it.
async fn rejected<A: ReceiverAccess<Family = Direct<()>>>(io: &mut A) -> usize {
    let mut state = State(1);
    let mut composed = io.compose(&mut state);
    state.0 = 2;
    poll_fn(|cx| composed.poll_access(cx, |a, _| Poll::Ready(*a.into_pin().get_mut().0))).await
}
fn main() {}

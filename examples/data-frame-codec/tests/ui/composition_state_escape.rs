// expect: E0515
// facade-only
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{Compose, Direct, DirectView, ReceiverAccess};
use std::{future::{Future, poll_fn}, pin::Pin, task::Poll};
struct State(usize);
impl Compose<Direct<()>> for State {
    type Family = Direct<usize>;
    fn compose<'v, 'p>(self: Pin<&'v mut Self>, _: Pin<&'v mut DirectView<'p, ()>>) -> DirectView<'v, usize>
    where Direct<()>: 'p, Self::Family: 'v, 'p: 'v {
        DirectView(Pin::new(&mut self.get_mut().0))
    }
}
// The adapted access borrows the local state, so it cannot leave the function.
fn rejected<'a, A: ReceiverAccess<Family = Direct<()>>>(io: &'a mut A) -> impl Future<Output = usize> + 'a {
    let mut state = State(1);
    let mut composed = io.compose(&mut state);
    async move { poll_fn(|cx| composed.poll_access(cx, |a, _| Poll::Ready(*a.into_pin().get_mut().0))).await }
}
fn main() {}

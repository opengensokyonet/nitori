// expect: E0515
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{call,Compose,Direct,DirectView,Receiver,ReceiverExt};
use std::pin::Pin;
struct State(usize);
impl Compose<Direct<()>> for State {
 type Family=Direct<usize>;
 fn compose<'v>(self:Pin<&'v mut Self>,_:DirectView<'v,()>) -> DirectView<'v,usize>
 where Direct<()>:'v,Self::Family:'v {DirectView(Pin::new(&mut self.get_mut().0))}
}
#[call]
async fn value(io:Receiver<Direct<usize>>) -> usize {io.with(|a|*a.into_pin().get_mut().0).await}
#[call]
async fn rejected(io:Receiver<Direct<()>>) -> nitori_call::Child<Direct<()>,nitori_call::Routed<nitori_call::Composed<Receiver<Direct<()>>, &'static mut State>, Value>> {
 let mut state=State(1);
 io.compose(&mut state).value()
}
fn main() {}

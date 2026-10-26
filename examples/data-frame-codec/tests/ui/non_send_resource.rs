// expect: pass
// facade-only
// A resource that is not Send does not keep the call from being Send.
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{Call, Direct, ReceiverAccess, ReceiverFamily, call};
use std::{future::poll_fn, pin::Pin, rc::Rc, task::Poll};
trait Byte: ReceiverFamily {
    fn value<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> u8 where Self: 'v;
}
impl Byte for Direct<Rc<u8>> {
    fn value<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> u8 where Self: 'v { **view.get_mut().0 }
}
#[call]
async fn value<A: ReceiverAccess<Family: Byte>>(io: &mut A) -> u8 {
    poll_fn(|cx| io.poll_access(cx, |a, _| Poll::Ready(A::Family::value(a.into_pin())))).await
}
fn send<T: Send>(_: T) {}
fn main() { send(Call::<Direct<Rc<u8>>, _>::new(Value::new())); }

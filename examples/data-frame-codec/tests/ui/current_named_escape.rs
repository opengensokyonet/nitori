// expect: lifetime may not live long enough
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call;
#[call]
async fn bad(io: nitori_call::Receiver<nitori_call::Direct<usize>>)-> &'static usize {io.with(|__access| { let target = __access.into_pin().get_mut().0.as_mut(); target.get_mut() }).await}
fn main(){}

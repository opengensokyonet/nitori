// expect: call receiver must be Receiver<Family>
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call;
#[call]
async fn bad<T: ?Sized>(io:T) {}
fn main(){}

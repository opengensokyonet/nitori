// expect: cannot be sent between threads safely
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call;
use std::{rc::Rc,pin::Pin};
#[call]
async fn value(io:nitori_call::Receiver<nitori_call::Direct<Rc<u8>>>)->u8 {io.with(|__access| { let target = __access.into_pin().get_mut().0.as_mut(); **target }).await}
fn send(_:impl Send){}
fn main(){let mut target=Rc::new(3);send(target.value_unpin());}

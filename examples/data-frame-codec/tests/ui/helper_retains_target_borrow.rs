// expect: E0503
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call;
use std::pin::Pin;
#[call]
async fn value(io:nitori_call::Receiver<nitori_call::Direct<usize>>)->usize {io.with(|__access| { let target = __access.into_pin().get_mut().0.as_mut(); *target }).await}
fn main(){let mut target=0usize;let operation=target.value_unpin();target+=1;drop(operation);}

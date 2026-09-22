// expect: pass
// facade-only
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::{call,call_closure,CallOn};
use std::{future::Future,pin::{Pin,pin},task::{Context,Poll,Waker}};
#[call]
async fn bump(io:nitori_call::Target<nitori_call::Direct<usize>>)->usize {io.with(|__access| { let mut host = __access.into_pin().get_mut().0.as_mut(); {*host+=1;*host} }).await}
fn main(){
 let mut host=0usize;let mut cx=Context::from_waker(Waker::noop());
 {let mut future=pin!(host.bump_unpin());assert_eq!(future.as_mut().poll(&mut cx),Poll::Ready(1));}
 let mut operation=pin!(call_closure!(|io:nitori_call::Target<nitori_call::Direct<usize>>|{io.bump().await}));
 let _=operation.as_mut().poll_receiver(Pin::new(&mut host),&mut cx);
}

// expect: pass
// facade-only
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use sakuya_call::{call,call_closure,CallOn};
use std::{future::Future,pin::{Pin,pin},task::{Context,Poll,Waker}};
#[call]
async fn bump(io:Pin<&mut usize>)->usize {io.with(|mut host|{*host+=1;*host})}
fn main(){
 let mut host=0usize;let mut cx=Context::from_waker(Waker::noop());
 {let mut future=pin!(host.bump_unpin());assert_eq!(future.as_mut().poll(&mut cx),Poll::Ready(1));}
 let mut operation=pin!(call_closure!(|io:Pin<&mut usize>|{io.bump().await}));
 let _=operation.as_mut().poll_call(Pin::new(&mut host),&mut cx);
}

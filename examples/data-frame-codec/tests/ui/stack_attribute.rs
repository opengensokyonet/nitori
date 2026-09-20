// expect: attributes are outside the call macro safety boundary
#![feature(coroutines,coroutine_trait)]
use sakuya_call::call_closure;
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut usize>|{#[allow(unused)] let value=1;io.with(|target|*target+value)});}

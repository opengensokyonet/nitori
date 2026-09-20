// expect: lifetime may not live long enough
#![feature(coroutines,coroutine_trait)]
use sakuya_call::call_closure;
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut usize>|{let borrowed=io.with(|target|target);std::future::ready(()).await;borrowed});}

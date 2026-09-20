// expect: FnOnce
#![feature(coroutines,coroutine_trait)]
use nitori_call::call_closure;
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut usize>|{io.with({let closure=|_|{};closure})});}

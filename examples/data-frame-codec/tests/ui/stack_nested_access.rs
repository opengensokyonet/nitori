// expect: E0425
#![feature(coroutines,coroutine_trait)]
use nitori_call::call_closure;
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut usize>|{io.with(|target|io.with(|_|*target))});}

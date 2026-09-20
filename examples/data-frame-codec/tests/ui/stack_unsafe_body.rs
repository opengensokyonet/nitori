// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::call_closure;
unsafe fn forbidden()->usize{1}
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut usize>|{io.with(|_|forbidden())});}

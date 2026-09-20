// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::call_closure;
unsafe fn make_future()->impl std::future::Future<Output=usize>{std::future::ready(1)}
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut usize>|{make_future().await});}

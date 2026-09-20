// expect: E0133
#![feature(coroutines,coroutine_trait)]
use sakuya_call::call_closure;
unsafe fn callback()->impl for<'a> FnOnce(std::pin::Pin<&'a mut usize>)->usize {|target|*target}
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut usize>|{io.with(callback())});}

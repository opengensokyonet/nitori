// expect: virtual host type must be Pin<&mut T>
#![feature(coroutines,coroutine_trait)]
use sakuya_call::call_closure;
fn main(){let _=call_closure!(|io: std::pin::Pin<&usize>|{});}

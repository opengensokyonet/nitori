// expect: cannot be shared between threads safely
#![feature(coroutines,coroutine_trait)]
use sakuya_call::call_closure;
fn require_sync(_:impl Sync){}
fn main(){let capture=std::cell::Cell::new(7);require_sync(call_closure!(move |io: ::core::pin::Pin<&mut usize>|{yield capture.get();io.with(|target|*target)}));}

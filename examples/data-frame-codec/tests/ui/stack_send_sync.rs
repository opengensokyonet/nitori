// expect: pass
#![feature(coroutines,coroutine_trait)]
use sakuya_call::call_closure;
fn require_both(_:impl Send+Sync){}
fn main(){require_both(call_closure!(|io: ::core::pin::Pin<&mut std::rc::Rc<std::cell::Cell<usize>>>|{yield io.with(|target|target.get());io.with(|target|target.get())}));}

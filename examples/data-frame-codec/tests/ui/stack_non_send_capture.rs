// expect: cannot be sent between threads safely
#![feature(coroutines,coroutine_trait)]
use nitori_call::call_closure;
fn require_send(_:impl Send){}
fn main(){let capture=std::rc::Rc::new(7);require_send(call_closure!(move |io: ::core::pin::Pin<&mut usize>|{yield *capture;io.with(|target|*target)}));}

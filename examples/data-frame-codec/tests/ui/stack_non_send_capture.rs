// expect: cannot be sent between threads safely
#![feature(coroutines,coroutine_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call_closure;
fn require_send(_:impl Send){}
fn main(){let capture=std::rc::Rc::new(7);require_send(call_closure!(move |io: nitori_call::Target<nitori_call::Direct<usize>>|{yield *capture;io.with(|__access| { let target = __access.into_pin().get_mut().0.as_mut(); *target }).await}));}

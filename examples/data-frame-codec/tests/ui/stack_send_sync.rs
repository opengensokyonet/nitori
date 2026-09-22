// expect: pass
#![feature(coroutines,coroutine_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call_closure;
fn require_both(_:impl Send+Sync){}
fn main(){require_both(call_closure!(|io: nitori_call::Target<nitori_call::Direct<std::rc::Rc<std::cell::Cell<usize>>>>|{yield io.with(|__access| { let target = __access.into_pin().get_mut().0.as_mut(); target.get() }).await;io.with(|__access| { let target = __access.into_pin().get_mut().0.as_mut(); target.get() }).await}));}

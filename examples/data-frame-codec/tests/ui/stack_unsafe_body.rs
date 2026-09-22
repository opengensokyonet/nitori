// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call_closure;
unsafe fn forbidden()->usize{1}
fn main(){let _=call_closure!(|io: nitori_call::Target<nitori_call::Direct<usize>>|{io.with(|__access| { let _ = __access.into_pin().get_mut().0.as_mut(); forbidden() }).await});}

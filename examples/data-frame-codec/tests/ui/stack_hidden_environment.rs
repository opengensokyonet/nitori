// expect: E0425
#![feature(coroutines,coroutine_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call_closure;
fn main(){let _=call_closure!(|io: nitori_call::Receiver<nitori_call::Direct<usize>>|{io.with(|__access| { let _ = __access.into_pin().get_mut().0.as_mut(); __stack_environment }).await}); }

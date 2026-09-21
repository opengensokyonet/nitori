// expect: opaque macros are outside the call macro safety boundary
#![feature(coroutines,coroutine_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call_closure;
macro_rules! hidden_pause {()=>{yield nitori_call::__private::Suspend::Pending};}
fn main(){let _=call_closure!(|io: nitori_call::Receiver<nitori_call::Direct<usize>>|{hidden_pause!();io.with(|__access| { let target = __access.into_pin().get_mut().0.as_mut(); *target }).await});}

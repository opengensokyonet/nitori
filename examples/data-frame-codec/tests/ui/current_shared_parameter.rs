// expect: call receiver must be Receiver<Family>
#![feature(coroutines,coroutine_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call_closure;
fn main(){let _=call_closure!(|io: std::pin::Pin<&usize>|{});}

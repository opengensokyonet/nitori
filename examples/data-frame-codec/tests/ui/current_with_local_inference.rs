// expect: FnOnce
#![feature(coroutines,coroutine_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call_closure;
fn main(){let _=call_closure!(|io: nitori_call::Receiver<nitori_call::Direct<usize>>|{io.with({let closure=|_|{};closure}).await});}

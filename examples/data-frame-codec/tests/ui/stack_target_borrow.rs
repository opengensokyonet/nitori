// expect: lifetime may not live long enough
#![feature(coroutines,coroutine_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call_closure;
fn main(){let _=call_closure!(|io: nitori_call::Receiver<nitori_call::Direct<usize>>|{let borrowed=io.with(|__access| { let target = __access.into_pin().get_mut().0.as_mut(); target }).await;std::future::ready(()).await;borrowed});}

// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call_closure;
unsafe fn make_future()->impl std::future::Future<Output=usize>{std::future::ready(1)}
fn main(){let _=call_closure!(|io: nitori_call::Receiver<nitori_call::Direct<usize>>|{make_future().await});}

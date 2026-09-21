// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call_closure;
unsafe fn callback()->impl for<'a,'h> FnOnce(nitori_call::Access<'a,'h,nitori_call::Direct<usize>>)->usize {|access|*access.into_pin().get_mut().0}
fn main(){let _=call_closure!(|io: nitori_call::Receiver<nitori_call::Direct<usize>>|{io.with(callback()).await});}

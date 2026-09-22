// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call_closure;
struct Target;impl Target{fn take(&self,value:usize)->usize{value}}
unsafe fn value()->usize{1}
fn main(){let _=call_closure!(|io: nitori_call::Target<Target>|{io.with(|access| access.into_pin().get_mut().0.take(value())).await});}

nitori_call::family_receiver!(impl [] for Target);

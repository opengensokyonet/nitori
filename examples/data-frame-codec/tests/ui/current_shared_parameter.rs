// expect: call receiver must be Target<Family>
#![feature(coroutines,coroutine_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call_closure;
fn main(){let _=call_closure!(|io: std::pin::Pin<&usize>|{});}

// expect: E0133
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call;
unsafe fn forbidden(){}
#[call]
async fn bad(io: nitori_call::Target<nitori_call::Direct<usize>>){io.with(|__access| { let _ = __access.into_pin().get_mut().0.as_mut(); forbidden() }).await}
fn main(){}

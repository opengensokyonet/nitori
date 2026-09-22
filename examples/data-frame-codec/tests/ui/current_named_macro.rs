// expect: opaque macros are outside the call macro safety boundary
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call;
#[call]
async fn bad(io: nitori_call::Target<nitori_call::Direct<usize>>){assert!(true);}
fn main(){}

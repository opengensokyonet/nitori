// expect: duplicate yields option
// facade-only
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call;
use std::pin::Pin;
#[call(sync, yields = u8, yields = u16)]
async fn example(io: nitori_call::Target<nitori_call::Direct<()>>) {}
fn main() {}

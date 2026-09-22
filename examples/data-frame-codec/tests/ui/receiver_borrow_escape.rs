// expect: lifetime may not live long enough
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::{call, Target};
#[call]
async fn decode(io: nitori_call::Target<nitori_call::Direct<usize>>) -> usize { io.with(|__access| { let host = __access.into_pin().get_mut().0.as_mut(); *host }).await }
#[call] async fn rejected(io: nitori_call::Target<nitori_call::Direct<usize>>) -> usize { let borrowed=io.with(|access| access.into_pin().get_mut().0.as_mut().get_mut()).await; std::future::ready(()).await; *borrowed }
fn main() {}

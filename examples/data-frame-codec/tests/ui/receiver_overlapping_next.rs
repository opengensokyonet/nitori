// expect: cannot borrow `child` as mutable more than once
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::{call, Target};
#[call]
async fn decode(io: nitori_call::Target<nitori_call::Direct<usize>>) -> usize { io.with(|__access| { let host = __access.into_pin().get_mut().0.as_mut(); *host }).await }
#[call] async fn rejected(io: nitori_call::Target<nitori_call::Direct<usize>>) { let mut child=std::pin::pin!(io.decode()); let first=child.as_mut().next(); let second=child.as_mut().next(); first.await; second.await; }
fn main() {}

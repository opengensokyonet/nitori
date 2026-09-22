// expect: lifetime may not live long enough
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::{call, Target};
#[call(sync, yields = usize)]
async fn events(io: nitori_call::Target<nitori_call::Direct<usize>>) { yield io.with(|__access| { let host = __access.into_pin().get_mut().0.as_mut(); *host }).await; }
#[call]
async fn rejected(io: nitori_call::Target<nitori_call::Direct<usize>>) {
    let events = io.with(|access| access.into_pin().get_mut().0.as_mut().sync_events()).await;
    let mut events = std::pin::pin!(events);
    let _ = events.as_mut().next();
}
fn main() {}

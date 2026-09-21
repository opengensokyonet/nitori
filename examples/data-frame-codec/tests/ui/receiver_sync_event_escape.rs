// expect: lifetime may not live long enough
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Receiver};
#[call(sync, yields = usize)]
async fn events(io: Receiver<'_, usize>) { yield io.with(|host| *host); }
#[call]
async fn rejected(io: Receiver<'_, usize>) {
    let mut events = std::pin::pin!(io.sync_events());
    let _ = events.as_mut().next();
}
fn main() {}

// expect: lifetime may not live long enough
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Receiver};
#[call]
async fn decode(io: Receiver<'_, usize>) -> usize { io.with(|host| *host) }
#[call] async fn rejected(io: Receiver<'_, usize>) -> usize { let borrowed=io.as_ref().get_ref(); std::future::ready(()).await; *borrowed }
fn main() {}

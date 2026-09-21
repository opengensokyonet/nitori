// expect: no method named `next`
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Receiver};
#[call]
async fn decode(io: Receiver<'_, usize>) -> usize { io.with(|host| *host) }
#[call] async fn rejected(io: Receiver<'_, usize>) { let mut child=io.decode(); child.next().await; }
fn main() {}

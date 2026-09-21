// expect: cannot borrow `child` as mutable more than once
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Receiver};
#[call]
async fn decode(io: Receiver<'_, usize>) -> usize { io.with(|host| *host) }
#[call] async fn rejected(io: Receiver<'_, usize>) { let mut child=std::pin::pin!(io.decode()); let first=child.as_mut().next(); let second=child.as_mut().next(); first.await; second.await; }
fn main() {}

// expect: E0271
// facade-only
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::call;
use std::pin::Pin;
#[call(sync)]
async fn example(io: Pin<&mut ()>) { yield 1u8; }
fn main() {}

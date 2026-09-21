// expect: no method named `sync_example_unpin`
// facade-only
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call;
use std::pin::Pin;
#[call]
async fn example(io: nitori_call::Receiver<nitori_call::Direct<()>>) {}
fn main() { ().sync_example_unpin(); }

// expect: AsRef
// facade-only
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call;
use std::pin::Pin;
#[call(sync)]
async fn example<Host: AsRef<[u8]> + ?Sized>(io: nitori_call::Receiver<Host>) {}
fn main() { <() as ExampleExt>::sync_example_unpin(&mut ()); }

// expect: no method named `sync_example_unpin`
// facade-only
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
#[call]
async fn example(_io: &mut impl ReceiverAccess<Family: Sized>) {}
fn main() { ().sync_example_unpin(); }

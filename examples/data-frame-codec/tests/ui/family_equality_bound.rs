// expect: bound the family by a trait, as in `ReceiverAccess<Family: Read>`
// facade-only
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{Direct, ReceiverAccess, call};
#[call]
async fn exact(_io: &mut impl ReceiverAccess<Family = Direct<usize>>) {}
fn main() {}

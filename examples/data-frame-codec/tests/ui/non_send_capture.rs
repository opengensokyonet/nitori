// expect: cannot be sent between threads safely
// facade-only
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{Call, Direct, ReceiverAccess, call};
use std::rc::Rc;
#[call]
async fn captured(_io: &mut impl ReceiverAccess<Family: Sized>, value: Rc<usize>) -> usize { *value }
fn send<T: Send>(_: T) {}
fn main() { send(Call::<Direct<usize>, _>::new(Captured::new(Rc::new(3)))); }

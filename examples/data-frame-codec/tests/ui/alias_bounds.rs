// expect: the trait bound `NotClone: Clone` is not satisfied
// facade-only
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
struct NotClone;
#[call]
async fn echo<T: Clone>(_io: &mut impl ReceiverAccess<Family: Sized>, input: T) -> T { input }
fn main() {
    // Naming the operation keeps the function's bounds.
    let _: Option<Echo<NotClone>> = None;
}

// expect: the trait bound `NotClone: Clone` is not satisfied
// facade-only
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call;
use std::pin::Pin;
struct NotClone;
#[call]
async fn echo<T: Clone>(io: nitori_call::Target<nitori_call::Direct<usize>>, input: T) -> T { input }
fn main() {
    // Merely naming the alias must retain the original struct's bounds.
    let _: Option<Echo<NotClone>> = None;
}

// expect: `Error` is not declared by `Fill`; name the supertrait that declares it, as in `<Self as Read>::Error`
// facade-only
#![feature(impl_trait_in_assoc_type)]
use nitori_call::capability;
use nitori_io::Read;
use std::{pin::Pin, task::{Context, Poll}};
#[capability]
pub trait Fill: Read {
    fn poll_fill<'v>(
        view: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>>
    where
        Self: 'v;
}
fn main() {}

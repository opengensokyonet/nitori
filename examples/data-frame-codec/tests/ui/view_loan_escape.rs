// expect: lifetime may not live long enough
// facade-only
use nitori_call::{Direct,DirectView,ViewLoan};
use std::pin::Pin;
fn escape<'s,'p:'s>(view:Pin<&'s mut DirectView<'p,usize>>)->ViewLoan<'static,Direct<usize>> {
 ViewLoan::new(view)
}
fn main() {}

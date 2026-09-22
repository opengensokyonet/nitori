// expect: lifetime may not live long enough
// facade-only
use nitori_call::{Direct,ViewLoan};
fn escape<'s>(loan:&'s mut ViewLoan<'_,Direct<usize>>)->&'s mut usize {
 loan.with(|access|access.into_pin().get_mut().0.as_mut().get_mut())
}
fn main() {}

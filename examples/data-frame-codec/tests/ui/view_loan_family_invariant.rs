// expect: lifetime may not live long enough
// facade-only
use nitori_call::{Direct,ViewLoan};
fn shorten<'s,'data:'s>(loan:ViewLoan<'s,Direct<&'data str>>)->ViewLoan<'s,Direct<&'s str>> {
 loan
}
fn main() {}

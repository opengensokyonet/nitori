#![feature(type_alias_impl_trait)]

type Inner = impl Sized;
#[define_opaque(Inner)]
fn inner() -> Inner { 8 }

fn outer() -> impl Fn() -> Inner { || inner() }
fn main() { let _ = outer()(); }

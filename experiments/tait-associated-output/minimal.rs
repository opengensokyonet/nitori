#![feature(type_alias_impl_trait)]

type Inner = impl Sized;
#[define_opaque(Inner)]
fn inner() -> Inner { 8 }

type Outer = impl Fn() -> Inner;
#[define_opaque(Outer)]
fn outer() -> Outer { || inner() }

fn main() { let _ = outer(); }

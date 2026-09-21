#![feature(type_alias_impl_trait)]

type Inner = impl Sized;
#[define_opaque(Inner)]
fn inner() -> Inner { 8 }

struct Value { _value: Inner }
type Outer = impl Fn() -> Value;
#[define_opaque(Outer)]
fn outer() -> Outer { || Value { _value: inner() } }

fn main() { let _ = outer()(); }

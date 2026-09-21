#![feature(type_alias_impl_trait, coroutines, coroutine_trait)]
use std::ops::Coroutine;
type Inner = impl Coroutine<(), Yield=(), Return=usize>;
#[define_opaque(Inner)] fn inner() -> Inner { #[coroutine] static |_: ()| { if false {yield;} 8 } }
type Outer = impl Coroutine<(), Yield=(), Return=Inner>;
#[define_opaque(Outer)] fn outer() -> Outer { #[coroutine] static |_: ()| { if false {yield;} inner() } }
fn main() {let _ = outer();}

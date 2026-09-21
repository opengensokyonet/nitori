// expect: E0277
#![feature(coroutines, coroutine_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use std::ops::Coroutine;
fn make<T>() -> impl for<'visit> Coroutine<&'visit mut T,Yield=(),Return=()> {
    #[coroutine]
    |mut target: &mut T| {let _=&mut *target;target=yield ();let _=&mut *target;}
}
fn main() {let _=make::<usize>();}

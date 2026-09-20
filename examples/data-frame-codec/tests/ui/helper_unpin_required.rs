// expect: Unpin
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::call;
use std::{marker::PhantomPinned,pin::Pin};
#[call]
async fn value(io:Pin<&mut PhantomPinned>)->usize {1}
fn main(){let mut target=PhantomPinned;let _=target.value_unpin();}

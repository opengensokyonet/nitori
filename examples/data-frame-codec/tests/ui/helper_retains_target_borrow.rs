// expect: E0503
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::call;
use std::pin::Pin;
#[call]
async fn value(io:Pin<&mut usize>)->usize {io.with(|target|*target)}
fn main(){let mut target=0usize;let operation=target.value_unpin();target+=1;drop(operation);}

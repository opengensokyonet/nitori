// expect: cannot be sent between threads safely
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use sakuya_call::call;
use std::{rc::Rc,pin::Pin};
#[call]
async fn value(io:Pin<&mut Rc<u8>>)->u8 {io.with(|target|**target)}
fn send(_:impl Send){}
fn main(){let mut target=Rc::new(3);send(target.value_unpin());}

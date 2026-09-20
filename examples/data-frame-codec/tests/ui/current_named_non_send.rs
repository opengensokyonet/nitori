// expect: cannot be sent between threads safely
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use sakuya_call::call;
use std::rc::Rc;
#[call]
async fn captured(io: ::core::pin::Pin<&mut usize>, value:Rc<usize>)->usize {*value}
fn send<T:Send>(_:T){}
fn main(){send(captured(Rc::new(3)));}

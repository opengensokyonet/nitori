// expect: cannot be sent between threads safely
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call;
use std::rc::Rc;
#[call]
async fn captured(io: nitori_call::Target<nitori_call::Direct<usize>>, value:Rc<usize>)->usize {*value}
fn send<T:Send>(_:T){}
fn main(){send(captured(Rc::new(3)));}

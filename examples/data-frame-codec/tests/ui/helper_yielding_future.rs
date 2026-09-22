// expect: pass
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call;
use std::{future::Future,pin::Pin};
#[call(yields = u8)]
async fn events(io:nitori_call::Target<nitori_call::Direct<usize>>)->usize {yield 1;2}
fn future(_:impl Future){}
fn main(){let mut target=0usize;future(target.events_unpin());}

// expect: E0133
#![feature(coroutines,coroutine_trait)]
use sakuya_call::call_closure;
use sakuya_call::CallOn;
use std::{convert::Infallible,ops::CoroutineState,pin::Pin,task::{Context,Poll}};
struct Child;
impl Child{unsafe fn new()->Self{Self}}

impl CallOn<()> for Child{type Yield=Infallible;type Return=();fn poll_call(self:Pin<&mut Self>,_:Pin<&mut ()>,_:&mut Context<'_>)->Poll<CoroutineState<Infallible,()>>{Poll::Ready(CoroutineState::Complete(()))}}
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut ()>|{io.child().await;});}

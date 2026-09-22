// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call_closure;
use nitori_call::CallOn;
use std::{convert::Infallible,ops::CoroutineState,pin::Pin,task::{Context,Poll}};
struct Child;
impl Child{unsafe fn new()->Self{Self}}

impl CallOn<nitori_call::Direct<()>> for Child{type Yield=Infallible;type Return=();fn poll_call<'v>(self:Pin<&mut Self>,_:Pin<&mut nitori_call::DirectView<'v,()>>,_:&mut Context<'_>)->Poll<CoroutineState<Infallible,()>> where nitori_call::Direct<()>:'v {Poll::Ready(CoroutineState::Complete(()))}}
fn main(){let _=call_closure!(|io: nitori_call::Target<nitori_call::Direct<()>>|{io.operation(Child::new()).await;});}

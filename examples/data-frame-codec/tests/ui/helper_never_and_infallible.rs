// expect: pass
#![feature(coroutine_trait,never_type)]
use sakuya_call::{BoundCall,CallOn,Stream};
use std::{convert::Infallible,future::Future,marker::PhantomData,ops::CoroutineState,pin::Pin,task::{Context,Poll}};
struct Complete<Yield>(PhantomData<fn()->Yield>);
impl<Yield> CallOn<usize> for Complete<Yield>{
 type Yield=Yield;type Return=usize;
 fn poll_call(self:Pin<&mut Self>,target:Pin<&mut usize>,_:&mut Context<'_>)->Poll<CoroutineState<Yield,usize>>{Poll::Ready(CoroutineState::Complete(*target))}
}
fn both<Yield>(_:impl Future<Output=usize>+Stream<Item=CoroutineState<Yield,usize>>){}
fn main(){let mut value=7;both(BoundCall::new(Pin::new(&mut value),Complete::<Infallible>(PhantomData)));both(BoundCall::new(Pin::new(&mut value),Complete::<!>(PhantomData)));}

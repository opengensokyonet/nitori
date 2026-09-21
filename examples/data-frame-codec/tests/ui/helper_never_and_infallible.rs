// expect: pass
#![feature(coroutine_trait,never_type)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::{BoundCall,CallOn,Stream};
use std::{convert::Infallible,future::Future,marker::PhantomData,ops::CoroutineState,pin::Pin,task::{Context,Poll}};
struct Complete<Yield>(PhantomData<fn()->Yield>);
impl<Yield> CallOn<nitori_call::Direct<usize>> for Complete<Yield>{
 type Yield=Yield;type Return=usize;
 fn poll_call<'v>(self:Pin<&mut Self>,target:Pin<&mut nitori_call::DirectView<'v,usize>>,_:&mut Context<'_>)->Poll<CoroutineState<Yield,usize>> where nitori_call::Direct<usize>:'v {Poll::Ready(CoroutineState::Complete(*target.get_mut().0))}
}
fn both<Yield>(_:impl Future<Output=usize>+Stream<Item=CoroutineState<Yield,usize>>){}
fn main(){let mut value = 7usize;both(BoundCall::new(Pin::new(&mut value),Complete::<Infallible>(PhantomData)));both(BoundCall::new(Pin::new(&mut value),Complete::<!>(PhantomData)));}

// expect: pass
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::{call, call_closure};
use nitori_call::CallOn;
use std::{pin::{Pin,pin}, task::{Context,Waker}};
trait Source { type Error; fn result(&self)->Result<usize,Self::Error>; }
impl Source for usize {type Error=(); fn result(&self)->Result<usize,()>{Ok(*self)}}
#[call(yields = &'data str)]
async fn generic<'data, Target: ?Sized, const OFFSET: usize>(io: nitori_call::Receiver<nitori_call::Direct<Target>>, text: &'data str) -> Result<usize,Target::Error>
where Target: Source {
    let unrelated = || 2;
    let offset = std::future::ready(unrelated()).await;
    yield text;
    io.with({let callback = |access: nitori_call::Access<'_,'_,nitori_call::Direct<Target>>| access.into_pin().get_mut().0.result(); callback}).await.map(|value|value + offset + OFFSET)
}
fn send_sync<T:Send+Sync>(_:&T){}
fn main(){
    let text=String::from("borrowed");
    let operation=generic::<usize,3>(&text); send_sync(&operation);
    let mut operation=pin!(operation);let mut target = 0usize;let mut cx=Context::from_waker(Waker::noop());
    let _=operation.as_mut().poll_host(Pin::new(&mut target),&mut cx);
    let mut inferred=pin!(call_closure!(|io| {std::future::ready(4).await}));
    let _=inferred.as_mut().poll_host(Pin::new(&mut target),&mut cx);
}

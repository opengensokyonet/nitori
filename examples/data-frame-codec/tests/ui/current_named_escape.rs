// expect: lifetime may not live long enough
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use sakuya_call::call;
#[call]
async fn bad(io: ::core::pin::Pin<&mut usize>)-> &'static usize {io.with(|target|target.get_mut())}
fn main(){}

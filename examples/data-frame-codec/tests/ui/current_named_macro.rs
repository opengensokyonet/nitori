// expect: opaque macros are outside the call macro safety boundary
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use sakuya_call::call;
#[call]
async fn bad(io: ::core::pin::Pin<&mut usize>){assert!(true);}
fn main(){}

// expect: E0133
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use sakuya_call::call;
unsafe fn forbidden(){}
#[call]
async fn bad(io: ::core::pin::Pin<&mut usize>){io.with(|_|forbidden())}
fn main(){}

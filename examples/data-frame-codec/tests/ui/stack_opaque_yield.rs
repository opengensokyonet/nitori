// expect: opaque macros are outside the call macro safety boundary
#![feature(coroutines,coroutine_trait)]
use nitori_call::call_closure;
macro_rules! hidden_pause {()=>{yield nitori_call::__private::Suspend::Pending};}
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut usize>|{hidden_pause!();io.with(|target|*target)});}

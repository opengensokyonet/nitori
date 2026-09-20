// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::call_closure;
struct Target;impl Target{fn take(&self,value:usize)->usize{value}}
unsafe fn value()->usize{1}
fn main(){let _=call_closure!(|io: ::core::pin::Pin<&mut Target>|{io.take(value())});}

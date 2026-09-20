// expect: virtual host type must be Pin<&mut T>
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::call;
#[call]
async fn bad<T: ?Sized>(io:T) {}
fn main(){}

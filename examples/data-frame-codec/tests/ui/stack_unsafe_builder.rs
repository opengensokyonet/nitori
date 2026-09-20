// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::__private::{ResumeEnv,build};
fn main(){let state=std::convert::identity(#[coroutine] static |_:ResumeEnv<()>|{});let _=build::<(),_,std::convert::Infallible>(state);}

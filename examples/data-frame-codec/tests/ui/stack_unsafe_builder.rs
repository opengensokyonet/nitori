// expect: E0133
#![feature(coroutines,coroutine_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::__private::{ResumeContext,build};
fn main(){let state=std::convert::identity(#[coroutine] static |_:ResumeContext<nitori_call::Direct<()>>|{});let _=build::<nitori_call::Direct<()>,_,std::convert::Infallible>(state);}

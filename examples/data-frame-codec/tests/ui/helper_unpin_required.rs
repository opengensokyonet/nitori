// expect: Unpin
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call;
use std::{marker::PhantomPinned,pin::Pin};
#[call]
async fn value(io:nitori_call::Target<Pinned>)->usize {1}
fn main(){let mut target=Pinned(PhantomPinned);let _=target.value_unpin();}

struct Pinned(PhantomPinned);
nitori_call::family_receiver!(impl [] for Pinned);

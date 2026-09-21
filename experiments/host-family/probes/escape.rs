#![feature(coroutine_trait)]

use host_family::{Call, Fixed};
use std::{ops::CoroutineState, pin::Pin, task::{Context, Poll}};

struct Escape;
impl Call<Fixed<u8>> for Escape {
    type Yield = ();
    type Return = &'static mut u8;

    fn poll<'host>(self: Pin<&mut Self>, host: Pin<&mut u8>, _: &mut Context<'_>)
        -> Poll<CoroutineState<(), Self::Return>> where Fixed<u8>: 'host {
        Poll::Ready(CoroutineState::Complete(host.get_mut()))
    }
}

fn main() {}

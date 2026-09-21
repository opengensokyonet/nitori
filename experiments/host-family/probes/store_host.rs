#![feature(coroutine_trait)]
use host_family::{Call, Fixed};
use std::{ops::CoroutineState, pin::Pin, task::{Context, Poll}};

struct Store<'saved> { saved: Option<Pin<&'saved mut u8>> }
impl Call<Fixed<u8>> for Store<'_> {
    type Yield = ();
    type Return = ();

    fn poll<'host>(self: Pin<&mut Self>, host: Pin<&mut u8>, _: &mut Context<'_>)
        -> Poll<CoroutineState<(), ()>> where Fixed<u8>: 'host {
        self.get_mut().saved = Some(host);
        Poll::Pending
    }
}

fn main() {}

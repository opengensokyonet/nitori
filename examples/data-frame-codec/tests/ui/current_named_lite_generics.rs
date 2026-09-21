// expect: pass
// facade-only
#![deny(warnings)]
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call;
use std::{pin::{Pin, pin}, task::{Context, Poll, Waker}, ops::CoroutineState};

struct Parameters(usize);
struct Operation(usize);
#[call]
async fn convert(_io: nitori_call::Receiver<nitori_call::Direct<usize>>, input: Parameters) -> Operation { Operation(input.0) }
#[call]
async fn length<T: AsRef<[u8]> + ?Sized>(io: nitori_call::Receiver<nitori_call::Direct<T>>) -> usize {
    io.with(|__access| { let host = __access.into_pin().get_mut().0.as_mut(); host.as_ref().get_ref().as_ref().len() }).await
}

mod nested {
    use super::*;

    // Const parameters, multiple bounds, lifetime bounds, and HRTBs must not
    // pass through pin-project-lite's restricted generic parser.
    #[call]
    pub(super) async fn inspect<'a, 'b: 'a, T: Clone + AsRef<[u8]> + 'a, const N: usize>(
        io: nitori_call::Receiver<nitori_call::Direct<usize>>, data: &'b T,
    ) -> usize
    where for<'c> &'c T: IntoIterator<Item = &'c u8> {
        let length = std::future::ready(data.as_ref().len()).await;
        io.with(|__access| { let mut host = __access.into_pin().get_mut().0.as_mut(); { *host += length + N; *host } }).await
    }

    mod deeper {
        use super::*;
        #[call]
        pub(in super::super) async fn add(io: nitori_call::Receiver<nitori_call::Direct<usize>>) -> usize {
            io.with(|__access| { let mut host = __access.into_pin().get_mut().0.as_mut(); { *host += 1; *host } }).await
        }
        #[call]
        pub(self) async fn local(io: nitori_call::Receiver<nitori_call::Direct<usize>>) -> usize {
            io.with(|__access| { let host = __access.into_pin().get_mut().0.as_mut(); *host }).await
        }
        #[call]
        pub(in crate::nested) async fn within(io: nitori_call::Receiver<nitori_call::Direct<usize>>) -> usize {
            io.local().await
        }
    }
    pub(super) fn visibility_checks() {
        let _ = deeper::Add::new();
        let _ = deeper::within();
    }
}
fn main() {
    nested::visibility_checks();
    let mut converted = pin!(Convert::new(Parameters(5)));
    let mut host = 0usize;
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(CoroutineState::Complete(result)) = converted.as_mut().poll_host(Pin::new(&mut host), &mut cx) else { panic!() };
    assert_eq!(result.0, 5);
    let mut bytes = vec![1u8, 2];
    let target: &mut (dyn AsRef<[u8]> + Unpin) = &mut bytes;
    let mut operation = pin!(Length::<dyn AsRef<[u8]> + Unpin>::new());
    assert_eq!(operation.as_mut().poll_host(Pin::new(&mut nitori_call::DirectView(Pin::new(target))), &mut cx), Poll::Ready(CoroutineState::Complete(2)));
    let bytes = vec![1u8, 2, 3];
    let mut host = 0usize;
    let mut cx = Context::from_waker(Waker::noop());
    let mut operation = pin!(nested::Inspect::<Vec<u8>, 4>::new(&bytes));
    assert_eq!(operation.as_mut().poll_host(Pin::new(&mut host), &mut cx), Poll::Ready(CoroutineState::Complete(7)));
}

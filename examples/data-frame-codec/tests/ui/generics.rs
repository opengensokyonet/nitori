// expect: pass
// facade-only
#![deny(warnings)]
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{Call, CallPoll, Direct, Receiver, ReceiverAccess, ReceiverFamily, call};
use std::{future::poll_fn, pin::{Pin, pin}, task::{Context, Poll, Waker}};

trait Number: ReceiverFamily {
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, amount: usize) -> usize where Self: 'v;
}
impl Number for Direct<usize> {
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, amount: usize) -> usize where Self: 'v {
        let mut host = view.get_mut().0.as_mut();
        *host += amount;
        *host
    }
}
async fn add<A: ReceiverAccess<Family: Number>>(io: &mut A, amount: usize) -> usize {
    poll_fn(|cx| io.poll_access(cx, |access, _| Poll::Ready(A::Family::add(access.into_pin(), amount)))).await
}

struct Parameters(usize);
struct Converted(usize);
#[call]
async fn convert(_io: &mut impl ReceiverAccess<Family: Number>, input: Parameters) -> Converted {
    Converted(input.0)
}

mod nested {
    use super::*;

    // Const parameters, multiple bounds, lifetime bounds and HRTBs.
    #[call]
    pub(super) async fn inspect<'a, 'b: 'a, T: Clone + AsRef<[u8]> + 'a, const N: usize>(
        io: &mut impl ReceiverAccess<Family: Number>, data: &'b T,
    ) -> usize
    where for<'c> &'c T: IntoIterator<Item = &'c u8> {
        let length = std::future::ready(data.as_ref().len()).await;
        add(io, length + N).await
    }

    mod deeper {
        use super::*;
        #[call]
        pub(in super::super) async fn increment(io: &mut impl ReceiverAccess<Family: Number>) -> usize {
            add(io, 1).await
        }
        #[call]
        pub(self) async fn current(io: &mut impl ReceiverAccess<Family: Number>) -> usize {
            add(io, 0).await
        }
        #[call]
        pub(in crate::nested) async fn within(io: &mut impl ReceiverAccess<Family: Number>) -> usize {
            io.current().await
        }
    }
    pub(super) fn visibility_checks() {
        let _ = deeper::Increment::new();
        let _ = deeper::Within::new();
    }

    #[call(module = ops)]
    pub(super) async fn twice(io: &mut impl ReceiverAccess<Family: Number>) -> usize {
        add(io, 1).await;
        add(io, 1).await
    }
}
fn poll<Op>(operation: Op, host: &mut usize) -> CallPoll<<Op as nitori_call::Operation<Direct<usize>>>::Output>
where
    Op: nitori_call::Operation<Direct<usize>> + for<'s> nitori_call::Start<'s, nitori_call::ReceiverSlot<'s, Direct<usize>>>,
{
    let call = pin!(Call::new(operation));
    call.poll_return_with(Some(pin!(Pin::new(host).view())), &mut Context::from_waker(Waker::noop()))
}
fn main() {
    nested::visibility_checks();
    let mut host = 0usize;
    let CallPoll::Ready(converted) = poll(Convert::new(Parameters(5)), &mut host) else { panic!() };
    assert_eq!(converted.0, 5);
    let bytes = vec![1u8, 2, 3];
    assert!(matches!(poll(nested::Inspect::<Vec<u8>, 4>::new(&bytes), &mut host), CallPoll::Ready(7)));
    assert!(matches!(poll(nested::ops::Twice::new(), &mut host), CallPoll::Ready(9)));
}

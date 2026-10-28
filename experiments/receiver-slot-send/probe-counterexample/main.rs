//! The operation that defeated the probe-based `Send` check: its future keeps
//! an `Rc` shared with the polling thread only when started with the real
//! slot. With the check on the real future, `Call: Send` must not hold.
#![feature(generic_const_exprs, const_type_name, impl_trait_in_assoc_type)]
#![allow(incomplete_features)]
use nitori_call::{Call, Direct, Emitter, Operation, ReceiverAccess, ReceiverSlot, Start};
use std::{cell::Cell, convert::Infallible, future::Future, pin::Pin, rc::Rc, task::{Context, Poll}};
type F = Direct<()>;
const fn real<A>() -> usize {
    let a = std::any::type_name::<A>().as_bytes();
    let b = std::any::type_name::<ReceiverSlot<'static, F>>().as_bytes();
    if a.len() != b.len() {
        return 0;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return 0;
        }
        i += 1;
    }
    1
}
trait Choose {
    type Value;
    fn make() -> Self::Value;
}
impl Choose for [(); 0] {
    type Value = ();
    fn make() {}
}
impl Choose for [(); 1] {
    type Value = Rc<Cell<u64>>;
    fn make() -> Self::Value {
        Rc::new(Cell::new(0))
    }
}
struct Holding<C: Choose>(C::Value);
impl<C: Choose> Future for Holding<C> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
}
struct Op;
impl Operation<F> for Op {
    type Output = ();
    type Yield = Infallible;
}
impl<'s, A: ReceiverAccess<Family = F>> Start<'s, A> for Op
where
    [(); real::<A>()]: Choose,
{
    type Fut = Holding<[(); real::<A>()]>;
    fn start(self, _: A, _: Emitter<'s, Infallible>) -> Self::Fut {
        Holding(<[(); real::<A>()] as Choose>::make())
    }
}
fn assert_send<T: Send>(_: &T) {}
fn main() {
    assert_send(&Call::<F, _>::new(Op));
}

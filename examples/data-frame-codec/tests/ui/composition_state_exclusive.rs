// expect: E0506
// facade-only
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{AccessPlan, Direct, DirectView, Lent, ProvideAccess, ReceiverAccess, ViewLoan, ViewProjection, project_access};
use std::{future::poll_fn, pin::Pin, task::Poll};
struct StateView;
impl ViewProjection<Lent<Direct<()>>> for StateView {
    type State = usize;
    type Family = Direct<usize>;
    fn project<'v>(_: ViewLoan<'v, Direct<()>>, state: &'v mut usize) -> DirectView<'v, usize>
    where Lent<Direct<()>>: 'v, Self::Family: 'v {
        DirectView(Pin::new(state))
    }
}

struct Projected<'a, A> {
    io: &'a mut A,
    state: &'a mut usize,
}
impl<A: ReceiverAccess<Family = Direct<()>>> ProvideAccess for Projected<'_, A> {
    type AccessFamily = Direct<usize>;
    type Lentness = A::Lentness;
    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        project_access::<_, StateView>(&mut self.io, self.state)
    }
}
// The state cannot change while the adapted access borrows it.
async fn rejected<A: ReceiverAccess<Family = Direct<()>>>(io: &mut A) -> usize {
    let mut state = 1;
    let mut composed = Projected { io, state: &mut state };
    state = 2;
    poll_fn(|cx| composed.poll_access(cx, |a, _| Poll::Ready(*a.into_pin().get_mut().0))).await
}
fn main() {}

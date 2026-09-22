#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{Direct, ReceiverFamily, Target, TargetExt, call, run_sync};
use std::pin::Pin;
#[call(sync)]
async fn value<F: ReceiverFamily>(io: Target<F>, n: usize) -> usize {
    let other = io;
    other.with(|_| n + 1).await
}
#[call(sync)]
async fn parent<F: ReceiverFamily>(io: Target<F>) -> usize {
    io.value(4).await
}
#[test]
fn real_receiver_and_host_extension() {
    let mut host = ();
    assert_eq!(host.sync_parent_unpin(), 5);
    assert_eq!(run_sync(Pin::new(&mut host), value::<Direct<()>>(8)), 9);
}

struct LocalState(usize);
impl nitori_call::Compose<Direct<()>> for LocalState {
    type Family = Direct<usize>;
    fn compose<'v, 'p>(
        self: Pin<&'v mut Self>,
        _: Pin<&'v mut nitori_call::DirectView<'p, ()>>,
    ) -> nitori_call::DirectView<'v, usize>
    where
        Direct<()>: 'p,
        'p: 'v,
        Self::Family: 'v,
    {
        nitori_call::DirectView(Pin::new(&mut self.get_mut().0))
    }
}
#[call(sync)]
async fn read_state(io: Target<Direct<usize>>) -> usize {
    io.with(|a| *a.into_pin().get_mut().0).await
}
#[call(sync)]
async fn owned_state(io: Target<Direct<()>>) -> usize {
    let mut description = io.compose(LocalState(9));
    let first = (&mut description).read_state().await;
    let second = (&mut description).call(ReadStateArguments::new()).await;
    let (root, state) = description.into_parts();
    root.with(|_| first + second + state.0).await
}
#[test]
fn owned_description_can_be_borrowed_reused_and_split() {
    assert_eq!(().sync_owned_state_unpin(), 27);
    assert_eq!(9usize.sync_read_state_unpin(), 9);
}

// A source type named State must remain visible inside the generated module.
struct State(usize);
#[call(sync)]
async fn state_argument(_io: Target<Direct<()>>, state: State) -> State {
    state
}
#[test]
fn generated_coroutine_state_does_not_shadow_author_types() {
    assert_eq!(().sync_state_argument_unpin(State(4)).0, 4);
}

#[call(sync)]
async fn hygienic<'__call, '__visit, __CallHost: Copy, __CallReceiver: Copy>(
    mut io: Target<Direct<()>>,
    marker: &'__call __CallHost,
    other: &'__visit __CallReceiver,
) -> (__CallHost, __CallReceiver) {
    (&mut io).with(|_| (*marker, *other)).await
}
#[test]
fn mutable_receivers_and_author_generic_names_are_preserved() {
    assert_eq!(().sync_hygienic_unpin(&4u8, &5u16), (4, 5));
}

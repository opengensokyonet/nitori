//! That the input of a locator only reads: what it takes is `Ref`, the
//! keys and the address of the lend point, the clock and wakers, and
//! capabilities made of them; never `Mut` (refused by what it touches,
//! [`InputReads`](super::locator::InputReads)), requests, a reaction
//! point or a contributed capability.
use frunk_core::hlist::{HCons, HNil};

use super::this::This;
use crate::{
    capability::{Capability, Contributed},
    need::{
        At, Cap, Clock, Mut, OneAtATime, OwnAddress, OwnKey, Plus, Ref, Requests, Under, WakerOf,
    },
    react::Reactions,
};

/// The need `Self`, in the input of a locator, only reads.
#[diagnostic::on_unimplemented(
    message = "the input of a locator takes `{Self}`; it may only read",
    note = "`Locator::Input` is lent to pick the node, beside what is taken there: it takes `Ref`, `OwnKey`, `OwnAddress`, `Clock`, `WakerOf` and capabilities made of them, never `Mut`, `Requests`, a reaction point or `Contributed`"
)]
pub trait InputNeed {}
impl InputNeed for HNil {}
impl<N: InputNeed, T: InputNeed> InputNeed for HCons<N, T> {}
macro_rules! reads {
    ($($t:ty $([$($g:ident),*])?;)*) => {$(
        impl$(<$($g),*>)? InputNeed for $t {}
    )*};
}
reads! {
    Ref<X> [X];
    // What it touches says that it is exclusive.
    Mut<X> [X];
    This<X> [X];
    OwnKey<K> [K];
    OwnAddress;
    Clock<C> [C];
    WakerOf<X> [X];
}
impl<B, T: InputNeed> InputNeed for At<B, T> {}
impl<A, T: InputNeed> InputNeed for Under<A, T> {}
impl<T: InputNeed> InputNeed for OneAtATime<T> {}
impl<Bs: InputNeed> InputNeed for Plus<Bs> {}
impl<X> InputNeed for Cap<X> where X: CapInput {}
impl<R> InputNeed for Requests<R> where R: PutInInput {}
impl<P, W> InputNeed for Reactions<P, W> where P: FiredInInput {}
impl<F, L> InputNeed for Contributed<F, L> where F: ContributedInInput {}

/// The needs of the capability `Self`, in the input of a locator, only
/// read.
#[diagnostic::on_unimplemented(
    message = "the input of a locator takes `Cap<{Self}>`; it may only read",
    note = "`Locator::Input` is lent to pick the node, beside what is taken there: a capability in it takes only `Ref`, `OwnKey`, `OwnAddress`, `Clock`, `WakerOf` and capabilities made of them"
)]
pub trait CapInput {}
impl<X: Capability> CapInput for X where X::Needs: InputNeed {}

/// Requests of the type `Self` put by the input of a locator: never.
#[diagnostic::on_unimplemented(
    message = "the input of a locator puts requests `{Self}`; it may only read",
    note = "`Locator::Input` is lent to pick the node, beside what is taken there: picking has no effect, and requests are put by what is taken at the node picked or by the need itself"
)]
pub trait PutInInput {}

/// The reaction point `Self` fired by the input of a locator: never.
#[diagnostic::on_unimplemented(
    message = "the input of a locator fires the reaction point `{Self}`; it may only read",
    note = "`Locator::Input` is lent to pick the node, beside what is taken there: picking has no effect, and a point is fired by what is taken at the node picked or by the need itself"
)]
pub trait FiredInInput {}

/// The family `Self` of a contributed capability taken by the input of a
/// locator: never.
#[diagnostic::on_unimplemented(
    message = "the input of a locator takes the contributed capability of the family `{Self}`; it may only read",
    note = "`Locator::Input` is lent to pick the node, beside what is taken there: it takes `Ref`, `OwnKey`, `OwnAddress`, `Clock`, `WakerOf` and capabilities made of them"
)]
pub trait ContributedInInput {}

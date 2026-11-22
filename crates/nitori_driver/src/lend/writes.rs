//! Which components a list of needs takes exclusive: what a lend may have
//! written, for reading deadlines again.
use alloc::vec::Vec;
use core::any::TypeId;

use frunk_core::hlist::{HCons, HNil};

use crate::{
    event::Publish,
    need::{At, Mut, MutList, Ref},
};

/// The components a list of needs takes exclusive, by type, for the
/// parts `P`.
pub trait Writes<P> {
    /// Push the type of every component taken exclusive onto `out`.
    fn writes(out: &mut Vec<TypeId>);
}
impl<P> Writes<P> for HNil {
    fn writes(_: &mut Vec<TypeId>) {}
}
impl<P, N: Writes<P>, T: Writes<P>> Writes<P> for HCons<N, T> {
    fn writes(out: &mut Vec<TypeId>) {
        N::writes(out);
        T::writes(out);
    }
}
impl<P, A, S, X: 'static> Writes<P> for At<A, S, Mut<X>> {
    fn writes(out: &mut Vec<TypeId>) {
        out.push(TypeId::of::<X>());
    }
}
impl<P, A, S, X> Writes<P> for At<A, S, Ref<X>> {
    fn writes(_: &mut Vec<TypeId>) {}
}
impl<P, A, S, L: TypeIds> Writes<P> for At<A, S, MutList<L>> {
    fn writes(out: &mut Vec<TypeId>) {
        L::type_ids(out);
    }
}
/// The types of a list of components.
pub trait TypeIds {
    /// Push every type onto `out`.
    fn type_ids(out: &mut Vec<TypeId>);
}
impl TypeIds for HNil {
    fn type_ids(_: &mut Vec<TypeId>) {}
}
impl<X: 'static, T: TypeIds> TypeIds for HCons<X, T> {
    fn type_ids(out: &mut Vec<TypeId>) {
        out.push(TypeId::of::<X>());
        T::type_ids(out);
    }
}
impl<P, A, S, E> Writes<P> for At<A, S, Publish<E>> {
    fn writes(_: &mut Vec<TypeId>) {}
}

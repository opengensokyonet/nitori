//! The need [`OwnKey`]: the key the node is kept under.
use frunk_core::{
    hlist::{HCons, HNil, Selector},
    indices::{Here as I0, There},
};

use super::{
    Build, Flat, KeyedList, KeyedPositions, KeyedShape, Locate, NeedList, OwnKeyCell, Pass1, Pass2,
    Places, Writes,
};
use crate::need::OwnKey;

/// The need `N` finds the key it reads in what is lent.
#[diagnostic::on_unimplemented(
    message = "the need `{N}` finds no key of its type",
    label = "the need `{N}` is not met here",
    note = "`OwnKey<K>` reads the key the node lent (or, in `build`, the node built) is kept under in its parent's store: the root has none, and `K` is the key of the place's storage kind"
)]
pub trait FindOwnKey<C, I, N> {}
impl<C, T, N> FindOwnKey<C, I0, N> for HCons<C, T> {}
impl<C, H, T: FindOwnKey<C, I, N>, I, N> FindOwnKey<C, There<I>, N> for HCons<H, T> {}

impl<K> Flat for OwnKey<K> {
    type Keyed = HNil;
}
impl<P, L: KeyedShape, K: 'static> NeedList<P, L> for OwnKey<K> {
    type View<'w, KP: KeyedList<P, L>> = &'w K;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<'a, Cx, U, K> Pass1<'a, Cx, U, ()> for OwnKey<K> {
    type Raw = ();
    type Rem = U;
    fn take(u: U) -> ((), U) {
        ((), u)
    }
}
impl<'a, Cx, Fr, K: 'a, FI> Pass2<'a, Cx, Fr, FI> for OwnKey<K>
where
    Fr: Selector<OwnKeyCell<'a, K>, FI>,
{
    type Raw = &'a K;
    fn get(f: &Fr) -> &'a K {
        f.get().0.0
    }
}
impl<'a, Cx, U, K: 'a, FI> Locate<Cx, U, FI> for OwnKey<K> where
    U: FindOwnKey<OwnKeyCell<'a, K>, FI, Self>
{
}
impl<Cx, U, K> Places<Cx, U, ()> for OwnKey<K> {
    type Out = HNil;
}
impl<Cx, K> KeyedPositions<Cx, ()> for OwnKey<K> {
    type KP = HNil;
}
impl<'a, P, L, KP, K: 'static> Build<'a, P, L, KP, (), &'a K> for OwnKey<K>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
{
    fn view(_: (), r2: &'a K) -> &'a K {
        r2
    }
}
impl<P, K> Writes<P> for OwnKey<K> {
    fn writes(_: &mut alloc::vec::Vec<core::any::TypeId>) {}
}

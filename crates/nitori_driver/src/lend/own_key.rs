//! The need [`OwnKey`]: the key the node is kept under.
use frunk_core::{
    hlist::{HCons, HNil, Selector},
    indices::{Here as I0, There},
};

use super::{
    Build, Cx, Flat, KeyedList, KeyedPositions, KeyedShape, Locate, NeedList, OwnKeyCell, Pass1,
    Pass2, Places, Writes,
};
use crate::need::{Anchor, At, Here, OwnKey};

/// The need `N` finds the key it reads in what is lent.
#[diagnostic::on_unimplemented(
    message = "the need `{N}` finds no key of its type",
    label = "the need `{N}` is not met here",
    note = "`OwnKey<K>` reads the key the node is kept under in its parent's store: the root has none, and `K` is the key of the place's storage kind"
)]
pub trait FindOwnKey<C, I, N> {}
impl<C, T, N> FindOwnKey<C, I0, N> for HCons<C, T> {}
impl<C, H, T: FindOwnKey<C, I, N>, I, N> FindOwnKey<C, There<I>, N> for HCons<H, T> {}

impl<A, K> Flat for At<A, Here, OwnKey<K>> {
    type Keyed = HNil;
}
impl<P, L: KeyedShape, A, K: 'static> NeedList<P, L> for At<A, Here, OwnKey<K>> {
    type View<'w, KP: KeyedList<P, L>> = &'w K;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<'a, Cx, U, A, K> Pass1<'a, Cx, U, ()> for At<A, Here, OwnKey<K>> {
    type Raw = ();
    type Rem = U;
    fn take(u: U) -> ((), U) {
        ((), u)
    }
}
impl<'a, Ch, P, Fr, A, K: 'a, AI, FI> Pass2<'a, Cx<Ch, P>, Fr, (AI, FI)> for At<A, Here, OwnKey<K>>
where
    A: Anchor<Ch, P, AI>,
    Fr: Selector<OwnKeyCell<'a, K>, FI>,
{
    type Raw = &'a K;
    fn get(f: &Fr) -> &'a K {
        f.get().0.0
    }
}
impl<'a, Ch, P, U, A, K: 'a, AI, FI> Locate<Cx<Ch, P>, U, (AI, FI)> for At<A, Here, OwnKey<K>>
where
    A: Anchor<Ch, P, AI>,
    U: FindOwnKey<OwnKeyCell<'a, K>, FI, Self>,
{
}
impl<Cx, U, A, K> Places<Cx, U, ()> for At<A, Here, OwnKey<K>> {
    type Out = HNil;
}
impl<Cx, A, K> KeyedPositions<Cx, ()> for At<A, Here, OwnKey<K>> {
    type KP = HNil;
}
impl<'a, P, L, KP, A, K: 'static> Build<'a, P, L, KP, (), &'a K> for At<A, Here, OwnKey<K>>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
{
    fn view(_: (), r2: &'a K) -> &'a K {
        r2
    }
}
impl<P, A, K> Writes<P> for At<A, Here, OwnKey<K>> {
    fn writes(_: &mut alloc::vec::Vec<core::any::TypeId>) {}
}

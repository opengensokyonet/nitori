//! The need [`Now`]: the instant the outbox holds.
use frunk_core::hlist::{HNil, Selector};

use super::{
    Build, Cx, Flat, KeyedList, KeyedPositions, KeyedShape, Locate, NeedList, Pass1, Pass2, Places,
    Writes,
};
use crate::{
    event::OutboxCell,
    need::{Anchor, At, Here, Now},
};

impl<A, I> Flat for At<A, Here, Now<I>> {
    type Keyed = HNil;
}
impl<P, L: KeyedShape, A, I: Copy + 'static> NeedList<P, L> for At<A, Here, Now<I>> {
    type View<'w, KP: KeyedList<P, L>> = I;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<'a, Cx, U, A, I> Pass1<'a, Cx, U, ()> for At<A, Here, Now<I>> {
    type Raw = ();
    type Rem = U;
    fn take(u: U) -> ((), U) {
        ((), u)
    }
}
impl<'a, Ch, P, Fr, A, I: Copy + 'a, AI, FI> Pass2<'a, Cx<Ch, P>, Fr, (AI, FI)>
    for At<A, Here, Now<I>>
where
    A: Anchor<Ch, P, AI>,
    Fr: Selector<OutboxCell<'a, I>, FI>,
{
    type Raw = I;
    fn get(f: &Fr) -> I {
        f.get().0.now()
    }
}
impl<Ch, P, U, A, I, AI> Locate<Cx<Ch, P>, U, AI> for At<A, Here, Now<I>> where A: Anchor<Ch, P, AI> {}
impl<Cx, U, A, I> Places<Cx, U, ()> for At<A, Here, Now<I>> {
    type Out = HNil;
}
impl<Cx, A, I> KeyedPositions<Cx, ()> for At<A, Here, Now<I>> {
    type KP = HNil;
}
impl<'a, P, L, KP, A, I: Copy + 'static> Build<'a, P, L, KP, (), I> for At<A, Here, Now<I>>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
{
    fn view(_: (), r2: I) -> I {
        r2
    }
}
impl<P, A, I> Writes<P> for At<A, Here, Now<I>> {
    fn writes(_: &mut alloc::vec::Vec<core::any::TypeId>) {}
}

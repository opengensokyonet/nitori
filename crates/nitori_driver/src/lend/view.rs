//! The view of a list of needs: its type from the list and the keyed
//! positions alone, and how it is built from the two passes' results.
use core::ops::Add;

use frunk_core::hlist::{HCons, HNil};

use super::{
    All, Keyed, KeyedItem, KeyedList, KeyedMany, KeyedPos, KeyedRef, KeyedRefItem, KeyedShape,
    Others, Refs,
};
use crate::need::{At, KeyedTag, Mut, MutList, Opt, OthersTag, Plain, Ref, StepTag, TagOf};

/// The keyed needs of a list of needs, in list order.
pub trait Flat {
    /// The keyed needs, an `HList` of [`KeyedDesc`](super::KeyedDesc)s.
    type Keyed: KeyedShape;
}
impl Flat for HNil {
    type Keyed = HNil;
}
impl<N: Flat, T: Flat> Flat for HCons<N, T>
where
    N::Keyed: Add<T::Keyed, Output: KeyedShape>,
{
    type Keyed = <N::Keyed as Add<T::Keyed>>::Output;
}
/// The keyed needs a view shape makes of an exclusive need of `X`.
pub trait TagFlat<X> {
    /// None or one.
    type Keyed: KeyedShape;
}
impl<X> TagFlat<X> for Plain {
    type Keyed = HNil;
}
impl<X> TagFlat<X> for Opt {
    type Keyed = HNil;
}
impl<X, K> TagFlat<X> for KeyedTag<K> {
    type Keyed = HCons<KeyedItem<All, K, X>, HNil>;
}
impl<X, K> TagFlat<X> for OthersTag<K> {
    type Keyed = HCons<KeyedItem<Others, K, X>, HNil>;
}
/// The keyed needs a view shape makes of a shared need of `X`.
pub trait TagFlatRef<X> {
    /// None or one.
    type Keyed: KeyedShape;
}
impl<X> TagFlatRef<X> for Plain {
    type Keyed = HNil;
}
impl<X> TagFlatRef<X> for Opt {
    type Keyed = HNil;
}
impl<X, K> TagFlatRef<X> for KeyedTag<K> {
    type Keyed = HCons<KeyedRefItem<All, K, X>, HNil>;
}
impl<X, K> TagFlatRef<X> for OthersTag<K> {
    type Keyed = HCons<KeyedRefItem<Others, K, X>, HNil>;
}
/// The keyed need a keyed view shape makes of a need of the components
/// `L`; there is none for a shape that is not by key.
pub trait TagFlatMany<L> {
    /// One.
    type Keyed: KeyedShape;
}
impl<L: Refs, K> TagFlatMany<L> for KeyedTag<K> {
    type Keyed = HCons<KeyedMany<All, K, L>, HNil>;
}
impl<L: Refs, K> TagFlatMany<L> for OthersTag<K> {
    type Keyed = HCons<KeyedMany<Others, K, L>, HNil>;
}
impl<A, S: StepTag<Plain, Tag: TagFlat<X>>, X> Flat for At<A, S, Mut<X>> {
    type Keyed = <TagOf<S> as TagFlat<X>>::Keyed;
}
impl<A, S: StepTag<Plain, Tag: TagFlatRef<X>>, X> Flat for At<A, S, Ref<X>> {
    type Keyed = <TagOf<S> as TagFlatRef<X>>::Keyed;
}
impl<A, S: StepTag<Plain, Tag: TagFlatMany<L>>, L> Flat for At<A, S, MutList<L>> {
    type Keyed = <TagOf<S> as TagFlatMany<L>>::Keyed;
}

type KHead<P, L, KP> = <KP as KeyedList<P, L>>::Head;
type KTail<P, L, KP> = <KP as KeyedList<P, L>>::Tail;

/// The view of a list of needs for the parts `P`. `L` is the list's keyed
/// needs from this need on and `KP` their positions: a keyed need takes
/// the head of both and leaves the tails ([`Rest`](NeedList::Rest),
/// [`RestPos`](NeedList::RestPos)) to the needs after it.
pub trait NeedList<P, L: KeyedShape> {
    /// The view.
    type View<'w, KP: KeyedList<P, L>>;
    /// The keyed needs after this one.
    type Rest: KeyedShape;
    /// Their positions.
    type RestPos<KP: KeyedList<P, L>>: KeyedList<P, Self::Rest>;
}
impl<P, L: KeyedShape> NeedList<P, L> for HNil {
    type View<'w, KP: KeyedList<P, L>> = HNil;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<P, L: KeyedShape, N, T> NeedList<P, L> for HCons<N, T>
where
    N: NeedList<P, L>,
    T: NeedList<P, N::Rest>,
{
    type View<'w, KP: KeyedList<P, L>> = HCons<N::View<'w, KP>, T::View<'w, N::RestPos<KP>>>;
    type Rest = T::Rest;
    type RestPos<KP: KeyedList<P, L>> = T::RestPos<N::RestPos<KP>>;
}
/// What a view shape makes of an exclusive need of `X`.
pub trait TagNeed<P, L: KeyedShape, X: 'static> {
    /// The view.
    type View<'w, KP: KeyedList<P, L>>;
    /// The keyed needs after this one.
    type Rest: KeyedShape;
    /// Their positions.
    type RestPos<KP: KeyedList<P, L>>: KeyedList<P, Self::Rest>;
}
impl<P, L: KeyedShape, X: 'static> TagNeed<P, L, X> for Plain {
    type View<'w, KP: KeyedList<P, L>> = &'w mut X;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<P, L: KeyedShape, X: 'static> TagNeed<P, L, X> for Opt {
    type View<'w, KP: KeyedList<P, L>> = Option<&'w mut X>;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<P, L: KeyedShape, X: 'static, K> TagNeed<P, L, X> for KeyedTag<K> {
    type View<'w, KP: KeyedList<P, L>> = Keyed<'w, P, L::Head, KHead<P, L, KP>>;
    type Rest = L::Tail;
    type RestPos<KP: KeyedList<P, L>> = KTail<P, L, KP>;
}
impl<P, L: KeyedShape, X: 'static, K> TagNeed<P, L, X> for OthersTag<K> {
    type View<'w, KP: KeyedList<P, L>> = Keyed<'w, P, L::Head, KHead<P, L, KP>>;
    type Rest = L::Tail;
    type RestPos<KP: KeyedList<P, L>> = KTail<P, L, KP>;
}
/// What a view shape makes of a shared need of `X`.
pub trait TagNeedRef<P, L: KeyedShape, X: 'static> {
    /// The view.
    type View<'w, KP: KeyedList<P, L>>;
    /// The keyed needs after this one.
    type Rest: KeyedShape;
    /// Their positions.
    type RestPos<KP: KeyedList<P, L>>: KeyedList<P, Self::Rest>;
}
impl<P, L: KeyedShape, X: 'static> TagNeedRef<P, L, X> for Plain {
    type View<'w, KP: KeyedList<P, L>> = &'w X;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<P, L: KeyedShape, X: 'static> TagNeedRef<P, L, X> for Opt {
    type View<'w, KP: KeyedList<P, L>> = Option<&'w X>;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<P, L: KeyedShape, X: 'static, K> TagNeedRef<P, L, X> for KeyedTag<K> {
    type View<'w, KP: KeyedList<P, L>> = KeyedRef<'w, P, L::Head, KHead<P, L, KP>>;
    type Rest = L::Tail;
    type RestPos<KP: KeyedList<P, L>> = KTail<P, L, KP>;
}
impl<P, L: KeyedShape, X: 'static, K> TagNeedRef<P, L, X> for OthersTag<K> {
    type View<'w, KP: KeyedList<P, L>> = KeyedRef<'w, P, L::Head, KHead<P, L, KP>>;
    type Rest = L::Tail;
    type RestPos<KP: KeyedList<P, L>> = KTail<P, L, KP>;
}
impl<P, L: KeyedShape, A, S, X: 'static> NeedList<P, L> for At<A, S, Mut<X>>
where
    S: StepTag<Plain, Tag: TagNeed<P, L, X>>,
{
    type View<'w, KP: KeyedList<P, L>> = <TagOf<S> as TagNeed<P, L, X>>::View<'w, KP>;
    type Rest = <TagOf<S> as TagNeed<P, L, X>>::Rest;
    type RestPos<KP: KeyedList<P, L>> = <TagOf<S> as TagNeed<P, L, X>>::RestPos<KP>;
}
impl<P, L: KeyedShape, A, S, X: 'static> NeedList<P, L> for At<A, S, Ref<X>>
where
    S: StepTag<Plain, Tag: TagNeedRef<P, L, X>>,
{
    type View<'w, KP: KeyedList<P, L>> = <TagOf<S> as TagNeedRef<P, L, X>>::View<'w, KP>;
    type Rest = <TagOf<S> as TagNeedRef<P, L, X>>::Rest;
    type RestPos<KP: KeyedList<P, L>> = <TagOf<S> as TagNeedRef<P, L, X>>::RestPos<KP>;
}
impl<P, L: KeyedShape, A, S, M> NeedList<P, L> for At<A, S, MutList<M>>
where
    S: StepTag<Plain, Tag: TagFlatMany<M>>,
{
    type View<'w, KP: KeyedList<P, L>> = Keyed<'w, P, L::Head, KHead<P, L, KP>>;
    type Rest = L::Tail;
    type RestPos<KP: KeyedList<P, L>> = KTail<P, L, KP>;
}

/// The view of a list of needs from the two passes' results `R1`, `R2`
/// and the positions `KP`.
pub trait Build<'a, P, L: KeyedShape, KP: KeyedList<P, L>, R1, R2>: NeedList<P, L> {
    /// Build the view.
    fn view(r1: R1, r2: R2) -> Self::View<'a, KP>;
}
impl<'a, P, L: KeyedShape, KP: KeyedList<P, L>> Build<'a, P, L, KP, HNil, HNil> for HNil {
    fn view(_: HNil, _: HNil) -> HNil {
        HNil
    }
}
impl<'a, P, L, KP, N, T, A1, A2, B1, B2> Build<'a, P, L, KP, HCons<A1, B1>, HCons<A2, B2>>
    for HCons<N, T>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
    N: Build<'a, P, L, KP, A1, A2>,
    T: Build<'a, P, N::Rest, N::RestPos<KP>, B1, B2>,
{
    fn view(r1: HCons<A1, B1>, r2: HCons<A2, B2>) -> Self::View<'a, KP> {
        HCons {
            head: N::view(r1.head, r2.head),
            tail: T::view(r1.tail, r2.tail),
        }
    }
}
/// The view of an exclusive need from pass 1's result.
pub trait TagNeedBuild<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static, R1>:
    TagNeed<P, L, X>
{
    /// Build the view.
    fn view(r1: R1) -> Self::View<'a, KP>;
}
impl<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static> TagNeedBuild<'a, P, L, KP, X, &'a mut X>
    for Plain
{
    fn view(r1: &'a mut X) -> &'a mut X {
        r1
    }
}
impl<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static>
    TagNeedBuild<'a, P, L, KP, X, Option<&'a mut X>> for Opt
{
    fn view(r1: Option<&'a mut X>) -> Option<&'a mut X> {
        r1
    }
}
impl<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static, K, R1> TagNeedBuild<'a, P, L, KP, X, R1>
    for KeyedTag<K>
where
    KHead<P, L, KP>: KeyedPos<P, L::Head, Raw<'a> = R1>,
{
    fn view(r1: R1) -> Self::View<'a, KP> {
        Keyed::new(r1)
    }
}
impl<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static, K, R1> TagNeedBuild<'a, P, L, KP, X, R1>
    for OthersTag<K>
where
    KHead<P, L, KP>: KeyedPos<P, L::Head, Raw<'a> = R1>,
{
    fn view(r1: R1) -> Self::View<'a, KP> {
        Keyed::new(r1)
    }
}
/// The view of a shared need from pass 2's result.
pub trait TagNeedRefBuild<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static, R2>:
    TagNeedRef<P, L, X>
{
    /// Build the view.
    fn view(r2: R2) -> Self::View<'a, KP>;
}
impl<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static> TagNeedRefBuild<'a, P, L, KP, X, &'a X>
    for Plain
{
    fn view(r2: &'a X) -> &'a X {
        r2
    }
}
impl<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static>
    TagNeedRefBuild<'a, P, L, KP, X, Option<&'a X>> for Opt
{
    fn view(r2: Option<&'a X>) -> Option<&'a X> {
        r2
    }
}
impl<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static, K, R2>
    TagNeedRefBuild<'a, P, L, KP, X, R2> for KeyedTag<K>
where
    KHead<P, L, KP>: KeyedPos<P, L::Head, Raw<'a> = R2>,
{
    fn view(r2: R2) -> Self::View<'a, KP> {
        KeyedRef::new(r2)
    }
}
impl<'a, P, L: KeyedShape, KP: KeyedList<P, L>, X: 'static, K, R2>
    TagNeedRefBuild<'a, P, L, KP, X, R2> for OthersTag<K>
where
    KHead<P, L, KP>: KeyedPos<P, L::Head, Raw<'a> = R2>,
{
    fn view(r2: R2) -> Self::View<'a, KP> {
        KeyedRef::new(r2)
    }
}
impl<'a, P, L, KP, A, S, X: 'static, R1> Build<'a, P, L, KP, R1, ()> for At<A, S, Mut<X>>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
    S: StepTag<Plain, Tag: TagNeedBuild<'a, P, L, KP, X, R1>>,
{
    fn view(r1: R1, _: ()) -> Self::View<'a, KP> {
        <TagOf<S> as TagNeedBuild<'a, P, L, KP, X, R1>>::view(r1)
    }
}
impl<'a, P, L, KP, A, S, X: 'static, R2> Build<'a, P, L, KP, (), R2> for At<A, S, Ref<X>>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
    S: StepTag<Plain, Tag: TagNeedRefBuild<'a, P, L, KP, X, R2>>,
{
    fn view(_: (), r2: R2) -> Self::View<'a, KP> {
        <TagOf<S> as TagNeedRefBuild<'a, P, L, KP, X, R2>>::view(r2)
    }
}
impl<'a, P, L, KP, A, S, M, R1> Build<'a, P, L, KP, R1, ()> for At<A, S, MutList<M>>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
    S: StepTag<Plain, Tag: TagFlatMany<M>>,
    KHead<P, L, KP>: KeyedPos<P, L::Head, Raw<'a> = R1>,
{
    fn view(r1: R1, _: ()) -> Self::View<'a, KP> {
        Keyed::new(r1)
    }
}

/// A list of needs with its keyed needs: what a view is made of.
pub trait NeedSet<P>: Flat + NeedList<P, <Self as Flat>::Keyed> {}
impl<P, N: Flat + NeedList<P, <N as Flat>::Keyed>> NeedSet<P> for N {}
/// The view of the list of needs `N` for the parts `P`, its keyed needs at
/// the positions `KP`.
pub type ViewOf<'w, N, P, KP> = <N as NeedList<P, <N as Flat>::Keyed>>::View<'w, KP>;

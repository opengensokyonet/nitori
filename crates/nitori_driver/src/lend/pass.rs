//! The two passes over the cells, and the positions of the keyed needs.
//!
//! Pass 1 plucks a cell for every exclusive need, in list order, from
//! what the earlier needs left; a second exclusive need of the same place
//! finds nothing. What is left is frozen to shared; pass 2 selects a cell
//! for every shared need, any number of times.
use core::{marker::PhantomData, ops::Add};

use frunk_core::hlist::{HCons, HNil, Plucker, Sculptor, Selector};

use super::{All, Cell, KeyedCell, KeyedManyP, KeyedP, Others, Refs, SharedKeyed};
use crate::{
    need::{
        Anchor, At, HasIn, KeyedOthersW, KeyedW, Mut, MutList, Opt, Plain, Ref, Steps, StepsKind,
        StepsPath, StepsWrap,
    },
    structure::{ChildStorage, FlatMut, KindFor, Node},
};

/// Where needs are lent: the chain up `Chain` and the parts `P`.
pub struct Cx<Chain, P>(PhantomData<fn() -> (Chain, P)>);

/// The cell an exclusive need takes, by the shape `Self` the steps give;
/// `B` is the kind reached, `P` the parts.
pub trait Shape<'a, X: 'a, B, P> {
    /// The cell.
    type Cell;
}
impl<'a, X: 'a, B, P> Shape<'a, X, B, P> for Plain {
    type Cell = &'a mut X;
}
impl<'a, X: 'a, B, P> Shape<'a, X, B, P> for Opt {
    type Cell = Option<&'a mut X>;
}
impl<'a, X: 'a, B: KindFor<P>, P: 'a, S: ChildStorage> Shape<'a, X, B, P> for KeyedW<S> {
    type Cell = KeyedCell<'a, All, S::Key, Node<B, P>>;
}
impl<'a, X: 'a, B: KindFor<P>, P: 'a, S: ChildStorage> Shape<'a, X, B, P> for KeyedOthersW<S> {
    type Cell = KeyedCell<'a, Others, S::Key, Node<B, P>>;
}
/// The cell a shared need takes, after freezing.
pub trait RefShape<'a, X: 'a, B, P> {
    /// The cell.
    type Cell: Copy;
}
impl<'a, X: 'a, B, P> RefShape<'a, X, B, P> for Plain {
    type Cell = &'a X;
}
impl<'a, X: 'a, B, P> RefShape<'a, X, B, P> for Opt {
    type Cell = Option<&'a X>;
}
impl<'a, X: 'a, B: KindFor<P>, P: 'a, S: ChildStorage> RefShape<'a, X, B, P> for KeyedW<S> {
    type Cell = SharedKeyed<'a, All, S::Key, Node<B, P>>;
}
impl<'a, X: 'a, B: KindFor<P>, P: 'a, S: ChildStorage> RefShape<'a, X, B, P> for KeyedOthersW<S> {
    type Cell = SharedKeyed<'a, Others, S::Key, Node<B, P>>;
}

/// The cell, tagged by its place, that the exclusive need `At<A, S, _>`
/// takes.
pub type CellOf<'a, X, A, S, Ch, P, AI, SI> = Cell<
    StepsPath<A, S, Ch, P, AI, SI>,
    <StepsWrap<A, S, Ch, P, AI, SI> as Shape<'a, X, StepsKind<A, S, Ch, P, AI, SI>, P>>::Cell,
>;
type ShapeCell<'a, X, A, S, Ch, P, AI, SI> =
    <StepsWrap<A, S, Ch, P, AI, SI> as Shape<'a, X, StepsKind<A, S, Ch, P, AI, SI>, P>>::Cell;
type RefCellOf<'a, X, A, S, Ch, P, AI, SI> = Cell<
    StepsPath<A, S, Ch, P, AI, SI>,
    <StepsWrap<A, S, Ch, P, AI, SI> as RefShape<'a, X, StepsKind<A, S, Ch, P, AI, SI>, P>>::Cell,
>;
type RefShapeCell<'a, X, A, S, Ch, P, AI, SI> =
    <StepsWrap<A, S, Ch, P, AI, SI> as RefShape<'a, X, StepsKind<A, S, Ch, P, AI, SI>, P>>::Cell;

/// Pass 1: the exclusive needs pluck their cells from `U`; `I` is
/// inferred.
pub trait Pass1<'a, Cx, U, I> {
    /// What the needs took.
    type Raw;
    /// What is left.
    type Rem;
    /// Take the cells.
    fn take(u: U) -> (Self::Raw, Self::Rem);
}
impl<'a, Cx, U> Pass1<'a, Cx, U, HNil> for HNil {
    type Raw = HNil;
    type Rem = U;
    fn take(u: U) -> (HNil, U) {
        (HNil, u)
    }
}
impl<'a, Cx, U, N, T, NI, TI> Pass1<'a, Cx, U, HCons<NI, TI>> for HCons<N, T>
where
    N: Pass1<'a, Cx, U, NI>,
    T: Pass1<'a, Cx, N::Rem, TI>,
{
    type Raw = HCons<N::Raw, T::Raw>;
    type Rem = T::Rem;
    fn take(u: U) -> (Self::Raw, T::Rem) {
        let (head, u) = N::take(u);
        let (tail, u) = T::take(u);
        (HCons { head, tail }, u)
    }
}
impl<'a, Ch, P, U, A, S, X: 'a, AI, SI, PI> Pass1<'a, Cx<Ch, P>, U, (AI, SI, PI)>
    for At<A, S, Mut<X>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: Shape<'a, X, StepsKind<A, S, Ch, P, AI, SI>, P>,
    U: Plucker<CellOf<'a, X, A, S, Ch, P, AI, SI>, PI>,
{
    type Raw = ShapeCell<'a, X, A, S, Ch, P, AI, SI>;
    type Rem = U::Remainder;
    fn take(u: U) -> (Self::Raw, Self::Rem) {
        let (c, rem) = u.pluck();
        (c.0, rem)
    }
}
impl<'a, Ch, P, U, A, S, L: 'a, AI, SI, PI> Pass1<'a, Cx<Ch, P>, U, (AI, SI, PI)>
    for At<A, S, MutList<L>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: Shape<'a, L, StepsKind<A, S, Ch, P, AI, SI>, P>,
    U: Plucker<CellOf<'a, L, A, S, Ch, P, AI, SI>, PI>,
{
    type Raw = ShapeCell<'a, L, A, S, Ch, P, AI, SI>;
    type Rem = U::Remainder;
    fn take(u: U) -> (Self::Raw, Self::Rem) {
        let (c, rem) = u.pluck();
        (c.0, rem)
    }
}
impl<'a, Cx, U, A, S, X> Pass1<'a, Cx, U, ()> for At<A, S, Ref<X>> {
    type Raw = ();
    type Rem = U;
    fn take(u: U) -> ((), U) {
        ((), u)
    }
}

/// What is left after pass 1, turned shared.
pub trait Freeze {
    /// The shared cells.
    type Out: Copy;
    /// Turn every cell shared.
    fn freeze(self) -> Self::Out;
}
impl Freeze for HNil {
    type Out = HNil;
    fn freeze(self) -> HNil {
        HNil
    }
}
impl<Path, C: FreezeOne, T: Freeze> Freeze for HCons<Cell<Path, C>, T> {
    type Out = HCons<Cell<Path, C::Out>, T::Out>;
    fn freeze(self) -> Self::Out {
        HCons {
            head: Cell::new(self.head.0.freeze()),
            tail: self.tail.freeze(),
        }
    }
}
/// One cell turned shared.
pub trait FreezeOne {
    /// The shared cell.
    type Out: Copy;
    /// Turn it shared.
    fn freeze(self) -> Self::Out;
}
impl<'a, X> FreezeOne for &'a mut X {
    type Out = &'a X;
    fn freeze(self) -> &'a X {
        self
    }
}
impl<'a, X> FreezeOne for Option<&'a mut X> {
    type Out = Option<&'a X>;
    fn freeze(self) -> Option<&'a X> {
        self.map(|x| &*x)
    }
}
impl<'a, W, K, N> FreezeOne for KeyedCell<'a, W, K, N> {
    type Out = SharedKeyed<'a, W, K, N>;
    fn freeze(self) -> Self::Out {
        self.into()
    }
}

/// Pass 2: the shared needs select their cells from the frozen `Fr`; `I`
/// is inferred.
pub trait Pass2<'a, Cx, Fr, I> {
    /// What the needs took.
    type Raw;
    /// Select the cells.
    fn get(f: &Fr) -> Self::Raw;
}
impl<'a, Cx, Fr> Pass2<'a, Cx, Fr, HNil> for HNil {
    type Raw = HNil;
    fn get(_: &Fr) -> HNil {
        HNil
    }
}
impl<'a, Cx, Fr, N, T, NI, TI> Pass2<'a, Cx, Fr, HCons<NI, TI>> for HCons<N, T>
where
    N: Pass2<'a, Cx, Fr, NI>,
    T: Pass2<'a, Cx, Fr, TI>,
{
    type Raw = HCons<N::Raw, T::Raw>;
    fn get(f: &Fr) -> Self::Raw {
        HCons {
            head: N::get(f),
            tail: T::get(f),
        }
    }
}
impl<'a, Cx, Fr, A, S, X> Pass2<'a, Cx, Fr, ()> for At<A, S, Mut<X>> {
    type Raw = ();
    fn get(_: &Fr) {}
}
impl<'a, Cx, Fr, A, S, L> Pass2<'a, Cx, Fr, ()> for At<A, S, MutList<L>> {
    type Raw = ();
    fn get(_: &Fr) {}
}
impl<'a, Ch, P, Fr, A, S, X: 'a, AI, SI, FI> Pass2<'a, Cx<Ch, P>, Fr, (AI, SI, FI)>
    for At<A, S, Ref<X>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: RefShape<'a, X, StepsKind<A, S, Ch, P, AI, SI>, P>,
    Fr: Selector<RefCellOf<'a, X, A, S, Ch, P, AI, SI>, FI>,
{
    type Raw = RefShapeCell<'a, X, A, S, Ch, P, AI, SI>;
    fn get(f: &Fr) -> Self::Raw {
        f.get().0
    }
}

// ---------------------------------------------------------------- positions of the keyed needs

/// The positions of the keyed needs of a list, in list order; `I` is
/// inferred. A keyed need's component must be in the data of the child
/// kind.
pub trait KeyedPositions<Cx, I> {
    /// The positions, an `HList` of [`KeyedP`] and [`KeyedManyP`].
    type KP;
}
impl<Cx> KeyedPositions<Cx, HNil> for HNil {
    type KP = HNil;
}
impl<Cx, N, T, NI, TI> KeyedPositions<Cx, HCons<NI, TI>> for HCons<N, T>
where
    N: KeyedPositions<Cx, NI>,
    T: KeyedPositions<Cx, TI>,
    N::KP: Add<T::KP>,
{
    type KP = <N::KP as Add<T::KP>>::Output;
}
impl<Ch, P, A, S, X, AI, SI, CI> KeyedPositions<Cx<Ch, P>, (AI, SI, CI)> for At<A, S, Mut<X>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: WrapPosition<X, StepsKind<A, S, Ch, P, AI, SI>, P, CI>,
{
    type KP = <StepsWrap<A, S, Ch, P, AI, SI> as WrapPosition<
        X,
        StepsKind<A, S, Ch, P, AI, SI>,
        P,
        CI,
    >>::KP;
}
impl<Ch, P, A, S, X, AI, SI, CI> KeyedPositions<Cx<Ch, P>, (AI, SI, CI)> for At<A, S, Ref<X>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: WrapPosition<X, StepsKind<A, S, Ch, P, AI, SI>, P, CI>,
{
    type KP = <StepsWrap<A, S, Ch, P, AI, SI> as WrapPosition<
        X,
        StepsKind<A, S, Ch, P, AI, SI>,
        P,
        CI,
    >>::KP;
}
impl<Ch, P, A, S, L, AI, SI, CI> KeyedPositions<Cx<Ch, P>, (AI, SI, CI)> for At<A, S, MutList<L>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: WrapManyPosition<L, StepsKind<A, S, Ch, P, AI, SI>, P, CI>,
{
    type KP = <StepsWrap<A, S, Ch, P, AI, SI> as WrapManyPosition<
        L,
        StepsKind<A, S, Ch, P, AI, SI>,
        P,
        CI,
    >>::KP;
}

/// The keyed position a shape gives a need of one component `X` of the
/// kind `B`: none unless the shape is keyed.
#[diagnostic::on_unimplemented(
    message = "the component `{X}` is not in the data of `{B}`",
    label = "a keyed need's component",
    note = "a keyed need takes a component of some part's share of the child kind"
)]
pub trait WrapPosition<X, B, P, CI> {
    /// The position, as a list of zero or one.
    type KP;
}
impl<X, B, P> WrapPosition<X, B, P, ()> for Plain {
    type KP = HNil;
}
impl<X, B, P> WrapPosition<X, B, P, ()> for Opt {
    type KP = HNil;
}
impl<X, B: KindFor<P, Data: HasIn<X, CI>>, P, S, CI> WrapPosition<X, B, P, CI> for KeyedW<S> {
    type KP = HCons<KeyedP<B, CI>, HNil>;
}
impl<X, B: KindFor<P, Data: HasIn<X, CI>>, P, S, CI> WrapPosition<X, B, P, CI> for KeyedOthersW<S> {
    type KP = HCons<KeyedP<B, CI>, HNil>;
}

type FlatStatic<B, P> = <<B as KindFor<P>>::Data as FlatMut<'static>>::Out;

/// The keyed position a keyed shape gives a need of the components `L` of
/// the kind `B`.
#[diagnostic::on_unimplemented(
    message = "the components `{L}` are not all in the data of `{B}`, or the step is not by key",
    label = "a `MutList` need",
    note = "`MutList` takes the children of a store by key (`ToChild<B, ByKey<K>>` or `ToChild<B, ByKeyOthers<K>>`), each of its components once from some part's share of `B`"
)]
pub trait WrapManyPosition<L, B, P, CI> {
    /// The position, as a list of one.
    type KP;
}
impl<L: Refs + 'static, B: KindFor<P>, P, S, CI> WrapManyPosition<L, B, P, CI> for KeyedW<S>
where
    B::Data: FlatMut<'static>,
    FlatStatic<B, P>: Sculptor<L::Mut<'static>, CI>,
{
    type KP = HCons<KeyedManyP<B, CI>, HNil>;
}
impl<L: Refs + 'static, B: KindFor<P>, P, S, CI> WrapManyPosition<L, B, P, CI> for KeyedOthersW<S>
where
    B::Data: FlatMut<'static>,
    FlatStatic<B, P>: Sculptor<L::Mut<'static>, CI>,
{
    type KP = HCons<KeyedManyP<B, CI>, HNil>;
}

//! Each need checked on its own, so that an error names the need.
//!
//! The passes thread what is left from one need to the next, so a failing
//! need shows up as an unmet bound of the whole list. These checks look
//! up every need against the whole of what is lent, each with its own
//! inference variables, and compare the places of the exclusive needs two
//! by two; the first error then names the need.
use core::ops::Add;

use frunk_core::{
    hlist::{HCons, HNil, Selector},
    indices::{Here as I0, There},
};

use super::{CellOf, Cx, Shape};
use crate::need::{
    Anchor, At, KeyedOthersW, KeyedW, Mut, MutList, Opt, Plain, Ref, Steps, StepsKind, StepsWrap,
};

/// The need `N` finds its cell `C` in what is lent.
#[diagnostic::on_unimplemented(
    message = "the need `{N}` finds nothing to take",
    label = "the need `{N}` is not met here",
    note = "no `{C}` on this structure: its anchor, steps or component is not there"
)]
pub trait FindCell<C, I, N> {}
impl<C, T, N> FindCell<C, I0, N> for HCons<C, T> {}
impl<C, H, T: FindCell<C, I, N>, I, N> FindCell<C, There<I>, N> for HCons<H, T> {}

/// A keyed need of every child ([`ByKey`](crate::need::ByKey)) finds its
/// store's cell.
#[diagnostic::on_unimplemented(
    message = "the need `{N}` finds no store to take whole by key",
    label = "the need `{N}` is not met here",
    note = "`ByKey` takes every child of a store off the chain (no node on the way up is kept in it); a store on the chain is taken with `ByKeyOthers`, or the anchor or step is not there (no `{C}`)"
)]
pub trait FindAll<C, I, N> {}
impl<C, T, N> FindAll<C, I0, N> for HCons<C, T> {}
impl<C, H, T: FindAll<C, I, N>, I, N> FindAll<C, There<I>, N> for HCons<H, T> {}
/// A keyed need of every child but the one on the way up
/// ([`ByKeyOthers`](crate::need::ByKeyOthers)) finds its store's cell.
#[diagnostic::on_unimplemented(
    message = "the need `{N}` finds no store on the chain to take by key",
    label = "the need `{N}` is not met here",
    note = "`ByKeyOthers` takes the other children of a store on the chain (one that keeps a node on the way up); a store off the chain is taken with `ByKey`, or the anchor or step is not there (no `{C}`)"
)]
pub trait FindOthers<C, I, N> {}
impl<C, T, N> FindOthers<C, I0, N> for HCons<C, T> {}
impl<C, H, T: FindOthers<C, I, N>, I, N> FindOthers<C, There<I>, N> for HCons<H, T> {}

/// The lookup a shape checks with; the keyed shapes say which word fits.
pub trait LocateIn<U, C, FI, N> {}
impl<U: FindCell<C, FI, N>, C, FI, N> LocateIn<U, C, FI, N> for Plain {}
impl<U: FindCell<C, FI, N>, C, FI, N> LocateIn<U, C, FI, N> for Opt {}
impl<S, U: FindAll<C, FI, N>, C, FI, N> LocateIn<U, C, FI, N> for KeyedW<S> {}
impl<S, U: FindOthers<C, FI, N>, C, FI, N> LocateIn<U, C, FI, N> for KeyedOthersW<S> {}

/// Each need of a list finds its cell in `U`, the whole of what is lent;
/// `I` is inferred.
pub trait Locate<Cx, U, I> {}
impl<Cx, U> Locate<Cx, U, HNil> for HNil {}
impl<Cx, U, N, T, NI, TI> Locate<Cx, U, HCons<NI, TI>> for HCons<N, T>
where
    N: Locate<Cx, U, NI>,
    T: Locate<Cx, U, TI>,
{
}
impl<'a, Ch, P, U, A, S, X: 'a, AI, SI, FI> Locate<Cx<Ch, P>, U, (AI, SI, FI)> for At<A, S, Mut<X>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: Shape<'a, X, StepsKind<A, S, Ch, P, AI, SI>, P>,
    StepsWrap<A, S, Ch, P, AI, SI>: LocateIn<U, CellOf<'a, X, A, S, Ch, P, AI, SI>, FI, Self>,
{
}
impl<'a, Ch, P, U, A, S, X: 'a, AI, SI, FI> Locate<Cx<Ch, P>, U, (AI, SI, FI)> for At<A, S, Ref<X>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: Shape<'a, X, StepsKind<A, S, Ch, P, AI, SI>, P>,
    StepsWrap<A, S, Ch, P, AI, SI>: LocateIn<U, CellOf<'a, X, A, S, Ch, P, AI, SI>, FI, Self>,
{
}
impl<'a, Ch, P, U, A, S, L: 'a, AI, SI, FI> Locate<Cx<Ch, P>, U, (AI, SI, FI)>
    for At<A, S, MutList<L>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: Shape<'a, L, StepsKind<A, S, Ch, P, AI, SI>, P>,
    StepsWrap<A, S, Ch, P, AI, SI>: LocateIn<U, CellOf<'a, L, A, S, Ch, P, AI, SI>, FI, Self>,
{
}

/// Every exclusive need of a list with the place of its cell in `U`, for
/// comparing them; `I` is inferred.
pub trait Places<Cx, U, I> {
    /// A list of `(place, need)`.
    type Out;
}
impl<Cx, U> Places<Cx, U, HNil> for HNil {
    type Out = HNil;
}
impl<Cx, U, N, T, NI, TI> Places<Cx, U, HCons<NI, TI>> for HCons<N, T>
where
    N: Places<Cx, U, NI>,
    T: Places<Cx, U, TI>,
    N::Out: Add<T::Out>,
{
    type Out = <N::Out as Add<T::Out>>::Output;
}
impl<'a, Ch, P, U, A, S, X: 'a, AI, SI, FI> Places<Cx<Ch, P>, U, (AI, SI, FI)> for At<A, S, Mut<X>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: Shape<'a, X, StepsKind<A, S, Ch, P, AI, SI>, P>,
    U: Selector<CellOf<'a, X, A, S, Ch, P, AI, SI>, FI>,
{
    type Out = HCons<(FI, Self), HNil>;
}
impl<'a, Ch, P, U, A, S, L: 'a, AI, SI, FI> Places<Cx<Ch, P>, U, (AI, SI, FI)>
    for At<A, S, MutList<L>>
where
    A: Anchor<Ch, P, AI>,
    S: Steps<A::Kind, A::Pre, Plain, SI>,
    StepsWrap<A, S, Ch, P, AI, SI>: Shape<'a, L, StepsKind<A, S, Ch, P, AI, SI>, P>,
    U: Selector<CellOf<'a, L, A, S, Ch, P, AI, SI>, FI>,
{
    type Out = HCons<(FI, Self), HNil>;
}
impl<Cx, U, A, S, X> Places<Cx, U, ()> for At<A, S, Ref<X>> {
    type Out = HNil;
}

/// Two exclusive needs take different places.
#[diagnostic::on_unimplemented(
    message = "the needs `{A}` and `{B}` take the same place",
    label = "`{A}` and `{B}` overlap",
    note = "an exclusive need (`Mut`, `MutList`) cannot share its place with another need"
)]
pub trait Distinct<I, J, A, B> {}
impl<J, A, B> Distinct<I0, There<J>, A, B> for () {}
impl<I, A, B> Distinct<There<I>, I0, A, B> for () {}
impl<I, J, A, B> Distinct<There<I>, There<J>, A, B> for () where (): Distinct<I, J, A, B> {}
/// Every two items of a list of `(place, need)` have different places.
pub trait Pairwise {}
impl Pairwise for HNil {}
impl<F, N, T: Pairwise + NoneAt<F, N>> Pairwise for HCons<(F, N), T> {}
/// No item of a list of `(place, need)` has the place `F`.
pub trait NoneAt<F, N> {}
impl<F, N> NoneAt<F, N> for HNil {}
impl<F, N, G, M, T: NoneAt<F, N>> NoneAt<F, N> for HCons<(G, M), T> where (): Distinct<F, G, N, M> {}

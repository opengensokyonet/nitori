//! The component of the lend point itself: what a component's operations
//! are lent first.
use alloc::vec::Vec;
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

use super::{
    Lend,
    check::{Check, Takes, TakesExclusive, Touch},
    plan::ExCol,
    tree::{Comps, Ctx, Elem, FlatData},
};
use crate::{
    id::True,
    need::{Need, Pos},
    structure::Shares,
};

/// The component `X` of the lend point itself, exclusive.
///
/// Not a word of the needs: a call takes a component on the way up
/// ([`Mut`](crate::need::Mut)); the driver lends a component's operations
/// the component of their own node, which another node on the way up may
/// have too.
pub struct This<X>(PhantomData<fn() -> X>);
impl<X: 'static> Need for This<X> {
    type View<'t, P: Pos> = &'t mut X;
}

/// The position of [`This`]: the component at `J` among the lend point's.
pub struct ThisPos<J>(PhantomData<fn() -> J>);
impl<J: 'static> Pos for ThisPos<J> {}

impl<C: Ctx, X: 'static, J: 'static> Lend<C, ThisPos<J>> for This<X>
where
    C::Parts: Shares<C::Kind>,
    <C::Parts as Shares<C::Kind>>::Data: FlatData,
    Comps<C::Kind, C::Parts>: Elem<X, J>,
    C::Route: 'static,
{
    type Cols = ExCol<C::Route, J, C::Fk, X>;
    fn build<'a>(hs: Vec<(C::Fk, &'a mut X)>, at: &C::Fk) -> &'a mut X
    where
        Self: 'a,
    {
        // Invariant: the lending found the lend point in the tree.
        hs.into_iter()
            .find(|(f, _)| f == at)
            .map(|(_, x)| x)
            .expect("the lend point is in the tree")
    }
}
impl<C, X, J> Check<C, ThisPos<J>> for This<X> {}
impl<C: Ctx, X, J> Takes<C, ThisPos<J>> for This<X> {
    type Touches = HCons<Touch<C::Route, X, True>, HNil>;
}
impl<X> TakesExclusive for This<X> {
    type Out = True;
}

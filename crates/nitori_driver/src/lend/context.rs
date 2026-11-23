//! The context of a lending, and the needs that read it: the clock the
//! driver read and the boxes requests are put in.
//!
//! The context is a list of pieces ([`Piece`]); a lending lays it out as
//! one row, a slot per piece, and each context need takes its piece from
//! the row in the first pass, as an exclusive column is taken from the
//! rows of the tables. The clock is shared and stays in its slot; a
//! request box is taken out of it, once, which the check of overlaps
//! ([`Asks`]) makes sure of.
use core::marker::PhantomData;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};
use nitori_clock::MonotonicClock;

use super::{
    Lend,
    check::{Check, Clash, DisjointFrom, Takes, Touch},
    list::{IndexEq, Or, Z},
    plan::{ExCol, NodeSet, PartPlan, Plan, Rebor, Run},
    plus::{AllDeep, Exclusive, HasTouch, IndexOf, Item, KeepUnless, Norm, Pick, SameItem, Union},
    table::KindTab,
    tree::Ctx,
};
use crate::{
    id::False,
    need::{Clock, LentClock, Pos, RequestBox, Requests},
};

// ------------------------------------------------------------------ the context

/// The context of a lending: a list of pieces, or `()` for none.
pub trait LendContext: 'static {
    /// One slot per piece, borrowed for `'a`.
    type Row<'a>;
    /// Every piece in its slot.
    fn row<'a>(&'a mut self) -> Self::Row<'a>;
}
impl LendContext for () {
    type Row<'a> = ();
    fn row(&mut self) {}
}
impl LendContext for HNil {
    type Row<'a> = HNil;
    fn row(&mut self) -> HNil {
        HNil
    }
}
impl<H: Piece, T: LendContext> LendContext for HCons<H, T> {
    type Row<'a> = HCons<Option<H::Hs<'a>>, T::Row<'a>>;
    fn row<'a>(&'a mut self) -> Self::Row<'a> {
        let HCons { head, tail } = self;
        HCons {
            head: Some(head.lend()),
            tail: tail.row(),
        }
    }
}

/// A piece of the context, and what a need that reads it is given.
pub trait Piece: 'static {
    /// What the need is given, borrowed for `'a`.
    type Hs<'a>;
    /// The piece, as it is put in its slot.
    fn lend<'a>(&'a mut self) -> Self::Hs<'a>;
    /// What a need is given of the piece in `slot`.
    fn take<'a>(slot: &mut Option<Self::Hs<'a>>) -> Self::Hs<'a>;
    /// What is given, lent again for a shorter time.
    fn rebor<'b, 'c: 'b>(hs: &'b mut Self::Hs<'c>) -> Self::Hs<'b>;
    /// What is given, for a shorter time.
    fn shorten<'a: 'c, 'c>(hs: Self::Hs<'a>) -> Self::Hs<'c>;
}
impl<C: MonotonicClock + 'static> Piece for LentClock<C> {
    type Hs<'a> = &'a LentClock<C>;
    fn lend(&mut self) -> &LentClock<C> {
        self
    }
    fn take<'a>(slot: &mut Option<Self::Hs<'a>>) -> Self::Hs<'a> {
        // Invariant: the clock is shared; no need takes it out of its slot.
        (*slot).expect("the clock stays in its slot")
    }
    fn rebor<'b, 'c: 'b>(hs: &'b mut &'c LentClock<C>) -> &'b LentClock<C> {
        hs
    }
    fn shorten<'a: 'c, 'c>(hs: &'a LentClock<C>) -> &'c LentClock<C> {
        hs
    }
}
impl<R: 'static> Piece for RequestBox<R> {
    type Hs<'a> = &'a mut RequestBox<R>;
    fn lend(&mut self) -> &mut RequestBox<R> {
        self
    }
    fn take<'a>(slot: &mut Option<Self::Hs<'a>>) -> Self::Hs<'a> {
        // Invariant: the check of overlaps refuses a list that takes the
        // box of one type of request twice.
        slot.take()
            .expect("a request box is taken once in a lending")
    }
    fn rebor<'b, 'c: 'b>(hs: &'b mut &'c mut RequestBox<R>) -> &'b mut RequestBox<R> {
        hs
    }
    fn shorten<'a: 'c, 'c>(hs: &'a mut RequestBox<R>) -> &'c mut RequestBox<R> {
        hs
    }
}

/// The context `Self` holds the piece `Pc`, at `I` (inferred).
#[diagnostic::on_unimplemented(
    message = "the context of the lending has no `{Pc}`",
    label = "a context need reads a piece this context does not hold",
    note = "`Clock<C>` reads the `LentClock<C>` of the context, `Requests<R>` its `RequestBox<R>`; the driver's context holds its own clock and a box for each type of request"
)]
pub trait HasPiece<Pc: Piece, I>: LendContext {
    /// The piece's slot.
    fn slot<'r, 'a>(row: &'r mut Self::Row<'a>) -> &'r mut Option<Pc::Hs<'a>>;
}
impl<Pc: Piece, T: LendContext> HasPiece<Pc, Here> for HCons<Pc, T> {
    fn slot<'r, 'a>(row: &'r mut Self::Row<'a>) -> &'r mut Option<Pc::Hs<'a>> {
        &mut row.head
    }
}
impl<Pc: Piece, H: Piece, T: HasPiece<Pc, I>, I> HasPiece<Pc, There<I>> for HCons<H, T> {
    fn slot<'r, 'a>(row: &'r mut Self::Row<'a>) -> &'r mut Option<Pc::Hs<'a>> {
        T::slot(&mut row.tail)
    }
}

// ------------------------------------------------------------------ the plan

/// The piece `Pc` of the context, at `I`.
pub struct CxCol<I, Pc>(PhantomData<fn() -> (I, Pc)>);

impl<I: 'static, Pc: Piece> Plan for CxCol<I, Pc> {
    type Hs<'a> = Pc::Hs<'a>;
}
impl<R, Pt: 'static, X, I: 'static, Pc: Piece> Run<R, Pt, X> for CxCol<I, Pc>
where
    R: KindTab<Pt, HNil>,
    X: HasPiece<Pc, I>,
{
    type Ex<'a> = Pc::Hs<'a>;
    type Store<'b> = ();
    fn take<'a>(_: &mut R::Table<'a>, cx: &mut X::Row<'a>) -> Pc::Hs<'a>
    where
        Self: 'a,
    {
        Pc::take(X::slot(cx))
    }
    fn gather<'a: 'b, 'b>(_: &'b R::Table<'a>) -> Self::Store<'b>
    where
        Self: 'a,
    {
    }
    fn handles<'a: 'c, 'b: 'c, 'c>(ex: Pc::Hs<'a>, _: &'c ()) -> Pc::Hs<'c>
    where
        Self: 'a,
    {
        Pc::shorten(ex)
    }
}
/// The clock is shared: every member of an `Under` reads it.
impl<M, I: 'static, C: MonotonicClock + 'static> PartPlan<M> for CxCol<I, LentClock<C>> {
    fn part<'a>(hs: &mut &'a LentClock<C>, _: &M) -> &'a LentClock<C>
    where
        Self: 'a,
    {
        hs
    }
}
impl<I: 'static, Pc: Piece> Rebor for CxCol<I, Pc> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut Pc::Hs<'c>) -> Pc::Hs<'b> {
        Pc::rebor(hs)
    }
}

// ------------------------------------------------------------------ in a `Plus`

impl<I, Pc> Norm for CxCol<I, Pc> {
    type Out = HCons<CxCol<I, Pc>, HNil>;
}
impl<I1: IndexEq<I2>, P1, I2, P2> SameItem<CxCol<I2, P2>> for CxCol<I1, P1> {
    type Out = I1::Out;
}
impl<I, Pc, Rt, J, F, X> SameItem<ExCol<Rt, J, F, X>> for CxCol<I, Pc> {
    type Out = False;
}
impl<I, Pc, Rt, J, F, X> SameItem<CxCol<I, Pc>> for ExCol<Rt, J, F, X> {
    type Out = False;
}
impl<I, Pc, Rt, F> SameItem<NodeSet<Rt, F>> for CxCol<I, Pc> {
    type Out = False;
}
impl<I, Pc, Rt, F> SameItem<CxCol<I, Pc>> for NodeSet<Rt, F> {
    type Out = False;
}
impl<I: 'static, Pc: Piece> Item for CxCol<I, Pc> {
    type Slot<'b, 'c>
        = Option<Pc::Hs<'b>>
    where
        'c: 'b;
    type Seen<'b, 'c>
        = ()
    where
        'c: 'b;
    fn slot<'b, 'c: 'b>(hs: &'b mut Pc::Hs<'c>) -> Option<Pc::Hs<'b>> {
        Some(Pc::rebor(hs))
    }
    fn seen<'b, 'c: 'b>(_: Option<Pc::Hs<'b>>) {}
}

/// The piece `Pc` is in the union at `I`.
pub trait CxAt<I>: Union {
    /// The piece.
    type Pc: Piece;
    /// What a need is given of it, from the slots.
    fn cx<'b, 'c: 'b>(slots: &mut Self::Slots<'b, 'c>) -> <Self::Pc as Piece>::Hs<'b>;
}
impl<J: 'static, Pc: Piece, T: Union> CxAt<Here> for HCons<CxCol<J, Pc>, T> {
    type Pc = Pc;
    fn cx<'b, 'c: 'b>(slots: &mut Self::Slots<'b, 'c>) -> Pc::Hs<'b> {
        Pc::take(&mut slots.head)
    }
}
impl<H: Item, T: CxAt<I>, I> CxAt<There<I>> for HCons<H, T> {
    type Pc = T::Pc;
    fn cx<'b, 'c: 'b>(slots: &mut Self::Slots<'b, 'c>) -> <T::Pc as Piece>::Hs<'b> {
        T::cx(&mut slots.tail)
    }
}
impl<U, J: 'static, Pc: Piece> Pick<U> for CxCol<J, Pc>
where
    U: IndexOf<CxCol<J, Pc>> + CxAt<<U as IndexOf<CxCol<J, Pc>>>::I, Pc = Pc>,
{
    type Ex<'b> = Pc::Hs<'b>;
    fn ex<'b, 'c: 'b>(slots: &mut U::Slots<'b, 'c>) -> Pc::Hs<'b> {
        <U as CxAt<_>>::cx(slots)
    }
    fn pick<'b, 'c: 'b>(ex: Pc::Hs<'b>, _: U::Seen<'b, 'c>) -> Pc::Hs<'b> {
        ex
    }
}

// ------------------------------------------------------------------ the needs

/// The position of a need that reads the piece of the context at `I`.
pub struct CxPos<I>(PhantomData<fn() -> I>);
impl<I: 'static> Pos for CxPos<I> {}

impl<C: Ctx, K: MonotonicClock + 'static, I: 'static> Lend<C, CxPos<I>> for Clock<K>
where
    C::Context: HasPiece<LentClock<K>, I>,
{
    type Cols = CxCol<I, LentClock<K>>;
    fn build<'a>(hs: &'a LentClock<K>, _: &C::Fk) -> &'a LentClock<K>
    where
        Self: 'a,
    {
        hs
    }
}
impl<C: Ctx, K: MonotonicClock + 'static, I> Check<C, CxPos<I>> for Clock<K> where
    C::Context: HasPiece<LentClock<K>, I>
{
}
impl<C, K, I> Takes<C, CxPos<I>> for Clock<K> {
    type Touches = HNil;
}

impl<C: Ctx, R: 'static, I: 'static> Lend<C, CxPos<I>> for Requests<R>
where
    C::Context: HasPiece<RequestBox<R>, I>,
{
    type Cols = CxCol<I, RequestBox<R>>;
    fn build<'a>(hs: &'a mut RequestBox<R>, _: &C::Fk) -> &'a mut RequestBox<R>
    where
        Self: 'a,
    {
        hs
    }
}
impl<C: Ctx, R: 'static, I> Check<C, CxPos<I>> for Requests<R>
where
    C::Context: HasPiece<RequestBox<R>, I>,
    C::Depth: OutsideMembers<R>,
{
}
impl<C, R, I> Takes<C, CxPos<I>> for Requests<R> {
    type Touches = HCons<Asks<I, R>, HNil>;
}

/// The lend point is not a member of an `Under` lent all at once, which
/// would each be given the one box of requests `R`.
#[diagnostic::on_unimplemented(
    message = "`Requests<{R}>` inside `Under` would be lent to every member at once, which share its box",
    label = "every member would borrow the same request box",
    note = "inside `Under<A, ..>`, `Requests<R>` is taken with the members `OneAtATime`, or outside the `Under`"
)]
pub trait OutsideMembers<R> {}
impl<R> OutsideMembers<R> for Z {}

// ------------------------------------------------------------------ overlap

/// What `Requests<R>` takes: the box of the context at `I`, exclusive.
pub struct Asks<I, R>(PhantomData<fn() -> (I, R)>);

/// Two takings of one box: the same requests twice.
#[diagnostic::on_unimplemented(
    message = "`Requests<{R}>` is taken twice in one lending",
    label = "these needs take the same request box",
    note = "a list takes one `Requests<R>` for each type of request; the branches of a `Plus`, taken one at a time, may each take it"
)]
pub trait AskedOnce<R> {}
impl<R> AskedOnce<R> for False {}

impl<R1, X1, E1, I, R> Clash<Asks<I, R>> for Touch<R1, X1, E1> {
    type Out = False;
}
impl<I, R, R2, X2, E2> Clash<Touch<R2, X2, E2>> for Asks<I, R> {
    type Out = False;
}
impl<I: IndexEq<J>, R, J, R2> Clash<Asks<J, R2>> for Asks<I, R> {
    type Out = I::Out;
}
impl<I, R, H, T> DisjointFrom<Asks<I, R>> for HCons<H, T>
where
    Asks<I, R>: Clash<H>,
    <Asks<I, R> as Clash<H>>::Out: AskedOnce<R>,
    T: DisjointFrom<Asks<I, R>>,
{
}

/// Whether a list of takings takes the box at `I`.
pub trait HasAsk<I> {
    /// [`True`](crate::id::True) or [`False`].
    type Out;
}
impl<I> HasAsk<I> for HNil {
    type Out = False;
}
impl<I, R2, X2, E2, T: HasAsk<I>> HasAsk<I> for HCons<Touch<R2, X2, E2>, T> {
    type Out = T::Out;
}
impl<I, J: IndexEq<I>, R, T: HasAsk<I>> HasAsk<I> for HCons<Asks<J, R>, T>
where
    J::Out: Or<T::Out>,
{
    type Out = <J::Out as Or<T::Out>>::Out;
}
impl<Rt, X, I, R, T: HasTouch<Rt, X>> HasTouch<Rt, X> for HCons<Asks<I, R>, T> {
    type Out = T::Out;
}
impl<I, R, T> Exclusive for HCons<Asks<I, R>, T>
where
    T: HasAsk<I> + Exclusive,
    <T as HasAsk<I>>::Out: KeepUnless<Asks<I, R>, <T as Exclusive>::Out>,
{
    type Out = <<T as HasAsk<I>>::Out as KeepUnless<Asks<I, R>, <T as Exclusive>::Out>>::Out;
}
impl<Sd, I, R, T: AllDeep<Sd>> AllDeep<Sd> for HCons<Asks<I, R>, T> {}

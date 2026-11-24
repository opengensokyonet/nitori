//! The context of a lending, and the needs that read it: the clock the
//! driver read and the boxes requests are put in.
//!
//! The context ([`Context`]) is a list of pieces ([`Piece`]) and the
//! order requests are put in; a lending lays it out as one row, a slot per
//! piece, and each context need copies its piece from the row in the first
//! pass. Every piece is shared: the clock is read, and a request box only
//! adds, numbering each request from the one order of the context, so no
//! context need overlaps another.
use alloc::{collections::VecDeque, vec::Vec};
use core::{cell::Cell, marker::PhantomData};

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};
use nitori_clock::MonotonicClock;

use super::{
    Lend,
    check::{Check, Takes, TakesExclusive},
    list::IndexEq,
    plan::{Copied, ExCol, NodeSet, PartPlan, Plan, Rebor, Run, ShCol},
    plus::{IndexOf, Item, Merge, Norm, Pick, SameItem, Union},
    rows::Span,
    table::KindTab,
    tree::Ctx,
};
use crate::{
    id::False,
    need::{Clock, ComponentWaker, LentClock, Pos, RequestBox, Requests, WakeBox, Wakes},
};

// ------------------------------------------------------------------ the context

/// The context of a lending: the pieces `L`, a list, and the order every
/// request of the lending is put in.
pub struct Context<L> {
    order: u64,
    pieces: L,
}
impl<L> Context<L> {
    /// The pieces `pieces`, with no request put in yet.
    pub(crate) fn new(pieces: L) -> Self {
        Self { order: 0, pieces }
    }
    /// The pieces.
    pub(crate) fn pieces_mut(&mut self) -> &mut L {
        &mut self.pieces
    }
}

/// The context of a lending: a [`Context`], or `()` for none.
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
impl<L: Pieces> LendContext for Context<L> {
    type Row<'a> = L::Row<'a>;
    fn row<'a>(&'a mut self) -> L::Row<'a> {
        let Context { order, pieces } = self;
        pieces.row(Cell::from_mut(order))
    }
}

/// The pieces of a context, a list.
pub trait Pieces: 'static {
    /// One slot per piece, borrowed for `'a`.
    type Row<'a>;
    /// Every piece in its slot; `order` numbers the requests.
    fn row<'a>(&'a mut self, order: &'a Cell<u64>) -> Self::Row<'a>;
}
impl Pieces for HNil {
    type Row<'a> = HNil;
    fn row(&mut self, _: &Cell<u64>) -> HNil {
        HNil
    }
}
impl<H: Piece, T: Pieces> Pieces for HCons<H, T> {
    type Row<'a> = HCons<H::Hs<'a>, T::Row<'a>>;
    fn row<'a>(&'a mut self, order: &'a Cell<u64>) -> Self::Row<'a> {
        let HCons { head, tail } = self;
        HCons {
            head: head.lend(order),
            tail: tail.row(order),
        }
    }
}

/// A piece of the context, and what a need that reads it is given: a
/// handle every need that reads it copies.
pub trait Piece: 'static {
    /// What the need is given, borrowed for `'a`.
    type Hs<'a>: Copy;
    /// The piece, as it is put in its slot; `order` numbers the requests.
    fn lend<'a>(&'a mut self, order: &'a Cell<u64>) -> Self::Hs<'a>;
    /// What is given, for a shorter time.
    fn shorten<'a: 'c, 'c>(hs: Self::Hs<'a>) -> Self::Hs<'c>;
}
impl<C: MonotonicClock + 'static> Piece for LentClock<C> {
    type Hs<'a> = &'a LentClock<C>;
    fn lend<'a>(&'a mut self, _: &'a Cell<u64>) -> &'a LentClock<C> {
        self
    }
    fn shorten<'a: 'c, 'c>(hs: &'a LentClock<C>) -> &'c LentClock<C> {
        hs
    }
}

/// The requests `R` put in during a lending, each with its place in the
/// order of the context.
///
/// The driver handles them from a queue: it moves the requests put in so
/// far to the queue, then takes them off one at a time, in the order of
/// the context across every list.
pub struct RequestList<R> {
    requests: Vec<(u64, R)>,
    queue: VecDeque<(u64, R)>,
}
impl<R> RequestList<R> {
    /// No requests.
    pub(crate) fn new() -> Self {
        Self {
            requests: Vec::new(),
            queue: VecDeque::new(),
        }
    }
    /// The requests put in, each with its place in the order, leaving the
    /// list empty.
    #[cfg(test)]
    pub(crate) fn take(&mut self) -> Vec<(u64, R)> {
        core::mem::take(&mut self.requests)
    }
    /// Move the requests put in so far behind those queued.
    pub(crate) fn enqueue(&mut self) {
        self.queue.extend(self.requests.drain(..));
    }
    /// The first request queued.
    pub(crate) fn first(&self) -> Option<&(u64, R)> {
        self.queue.front()
    }
    /// The places in the order of the requests queued.
    pub(crate) fn queued(&self) -> impl Iterator<Item = u64> + '_ {
        self.queue.iter().map(|(order, _)| *order)
    }
    /// Take the first request queued off the queue.
    pub(crate) fn pop(&mut self) -> Option<(u64, R)> {
        self.queue.pop_front()
    }
    /// Drop every request, put in or queued.
    pub(crate) fn clear(&mut self) {
        self.requests.clear();
        self.queue.clear();
    }
}
impl<R: 'static> Piece for RequestList<R> {
    type Hs<'a> = RequestBox<'a, R>;
    fn lend<'a>(&'a mut self, order: &'a Cell<u64>) -> RequestBox<'a, R> {
        RequestBox::new(Cell::from_mut(&mut self.requests), order)
    }
    fn shorten<'a: 'c, 'c>(hs: RequestBox<'a, R>) -> RequestBox<'c, R> {
        hs
    }
}

/// The wakes at the priority `P` put in during a lending, in the order
/// they were put in.
pub struct WakeList<P> {
    wakes: Vec<(ComponentWaker, P)>,
}
impl<P> WakeList<P> {
    /// No wakes.
    pub(crate) fn new() -> Self {
        Self { wakes: Vec::new() }
    }
    /// The wakes put in, in order, leaving the list empty.
    pub(crate) fn take(&mut self) -> Vec<(ComponentWaker, P)> {
        core::mem::take(&mut self.wakes)
    }
    /// The wakes put in, in order, swapped with those in `other`.
    pub(crate) fn swap(&mut self, other: &mut Vec<(ComponentWaker, P)>) {
        core::mem::swap(&mut self.wakes, other);
    }
}
impl<P: 'static> Piece for WakeList<P> {
    type Hs<'a> = WakeBox<'a, P>;
    fn lend<'a>(&'a mut self, _: &'a Cell<u64>) -> WakeBox<'a, P> {
        WakeBox::new(Cell::from_mut(&mut self.wakes))
    }
    fn shorten<'a: 'c, 'c>(hs: WakeBox<'a, P>) -> WakeBox<'c, P> {
        hs
    }
}

/// The context `Self` holds the piece `Pc`, at `I` (inferred).
#[diagnostic::on_unimplemented(
    message = "the context of the lending has no `{Pc}`",
    label = "a context need reads a piece this context does not hold",
    note = "`Clock<C>` reads the `LentClock<C>` of the context, `Requests<R>` its `RequestList<R>`, `Wakes<P>` its `WakeList<P>`; the driver's context holds its own clock, a list for each type of request and the list of wakes at the composition's priority"
)]
pub trait HasPiece<Pc: Piece, I>: LendContext {
    /// The piece, from its slot.
    fn piece<'a>(row: &Self::Row<'a>) -> Pc::Hs<'a>;
}
impl<Pc: Piece, L: PieceIn<Pc, I>, I> HasPiece<Pc, I> for Context<L> {
    fn piece<'a>(row: &L::Row<'a>) -> Pc::Hs<'a> {
        L::piece(row)
    }
}
/// The pieces `Self` hold the piece `Pc`, at `I` (inferred).
#[diagnostic::on_unimplemented(
    message = "the context of the lending has no `{Pc}`",
    label = "a context need reads a piece this context does not hold",
    note = "`Clock<C>` reads the `LentClock<C>` of the context, `Requests<R>` its `RequestList<R>`, `Wakes<P>` its `WakeList<P>`; the driver's context holds its own clock, a list for each type of request and the list of wakes at the composition's priority"
)]
pub trait PieceIn<Pc: Piece, I>: Pieces {
    /// The piece, from its slot.
    fn piece<'a>(row: &Self::Row<'a>) -> Pc::Hs<'a>;
}
impl<Pc: Piece, T: Pieces> PieceIn<Pc, Here> for HCons<Pc, T> {
    fn piece<'a>(row: &Self::Row<'a>) -> Pc::Hs<'a> {
        row.head
    }
}
impl<Pc: Piece, H: Piece, T: PieceIn<Pc, I>, I> PieceIn<Pc, There<I>> for HCons<H, T> {
    fn piece<'a>(row: &Self::Row<'a>) -> Pc::Hs<'a> {
        T::piece(&row.tail)
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
    fn mark(_: &mut R::Table<'_>) {}
    fn take<'a>(_: &mut R::Table<'a>, cx: &mut X::Row<'a>) -> Pc::Hs<'a>
    where
        Self: 'a,
    {
        X::piece(cx)
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
/// A piece is shared: every member of an `Under` copies it.
impl<MRt, I: 'static, Pc: Piece> PartPlan<MRt> for CxCol<I, Pc> {
    type Left<'a> = Pc::Hs<'a>;
    fn left<'a>(hs: Pc::Hs<'a>) -> Pc::Hs<'a>
    where
        Self: 'a,
    {
        hs
    }
    fn part<'a>(hs: &mut Pc::Hs<'a>, _: Span) -> Pc::Hs<'a>
    where
        Self: 'a,
    {
        *hs
    }
}
impl<MRt, I: 'static, Pc: Piece> Rebor<MRt> for CxCol<I, Pc> {
    fn rebor<'b, 'c: 'b>(hs: &'b mut Pc::Hs<'c>, _: Span) -> Pc::Hs<'b> {
        Pc::shorten(*hs)
    }
}
impl<I: 'static, Pc: Piece> Copied for CxCol<I, Pc> {
    fn copied<'b, 'c: 'b>(hs: &'b Pc::Hs<'c>) -> Pc::Hs<'c> {
        *hs
    }
    fn shortened<'b, 'c: 'b>(hs: &'b Pc::Hs<'c>) -> Pc::Hs<'b> {
        Pc::shorten(*hs)
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
impl<I, Pc, Rt, J, F, X> SameItem<ShCol<Rt, J, F, X>> for CxCol<I, Pc> {
    type Out = False;
}
impl<I, Pc, Rt, J, F, X> SameItem<CxCol<I, Pc>> for ShCol<Rt, J, F, X> {
    type Out = False;
}
impl<I, Pc, B> Merge<B> for CxCol<I, Pc> {
    type Out = Self;
}
impl<I, Pc, Rt, F, A> SameItem<NodeSet<Rt, F, A>> for CxCol<I, Pc> {
    type Out = False;
}
impl<I, Pc, Rt, F, A> SameItem<CxCol<I, Pc>> for NodeSet<Rt, F, A> {
    type Out = False;
}
impl<I: 'static, Pc: Piece> Item for CxCol<I, Pc> {
    type Slot<'b, 'c>
        = Pc::Hs<'b>
    where
        'c: 'b;
    type Seen<'b, 'c>
        = ()
    where
        'c: 'b;
    fn slot<'b, 'c: 'b>(hs: &'b mut Pc::Hs<'c>) -> Pc::Hs<'b> {
        Pc::shorten(*hs)
    }
    fn seen<'b, 'c: 'b>(_: Pc::Hs<'b>) {}
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
        slots.head
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
    C::Context: HasPiece<RequestList<R>, I>,
{
    type Cols = CxCol<I, RequestList<R>>;
    fn build<'a>(hs: RequestBox<'a, R>, _: &C::Fk) -> RequestBox<'a, R>
    where
        Self: 'a,
    {
        hs
    }
}
impl<C: Ctx, R: 'static, I> Check<C, CxPos<I>> for Requests<R> where
    C::Context: HasPiece<RequestList<R>, I>
{
}
/// Putting in only adds: it takes nothing another need could overlap.
impl<C, R, I> Takes<C, CxPos<I>> for Requests<R> {
    type Touches = HNil;
}

// ------------------------------------------------------------------ exclusive

/// Neither context need takes anything exclusive.
impl<C> TakesExclusive for Clock<C> {
    type Out = False;
}
impl<R> TakesExclusive for Requests<R> {
    type Out = False;
}
impl<P> TakesExclusive for Wakes<P> {
    type Out = False;
}

impl<C: Ctx, P: 'static, I: 'static> Lend<C, CxPos<I>> for Wakes<P>
where
    C::Context: HasPiece<WakeList<P>, I>,
{
    type Cols = CxCol<I, WakeList<P>>;
    fn build<'a>(hs: WakeBox<'a, P>, _: &C::Fk) -> WakeBox<'a, P>
    where
        Self: 'a,
    {
        hs
    }
}
impl<C: Ctx, P: 'static, I> Check<C, CxPos<I>> for Wakes<P> where
    C::Context: HasPiece<WakeList<P>, I>
{
}
/// Waking only adds: it takes nothing another need could overlap.
impl<C, P, I> Takes<C, CxPos<I>> for Wakes<P> {
    type Touches = HNil;
}

//! The kinds of call and the family they are lent through: a kind's needs,
//! operation and start, and the loan a poll lends.
use core::{convert::Infallible, marker::PhantomData, pin::Pin};

use nitori_call::{Access, LentSlot, Operation, ReceiverFamily, Start};

use super::{Composition, PriorityOf, host::CallHost};
use crate::{
    need::{Need, Pos, View},
    structure::NodeKind,
};

/// A kind of call lent at the nodes of the kind `K`, under the
/// composition `Q`: its needs, the operation of its `#[call]` function,
/// and how a new node's call starts.
///
/// The kind is the struct `#[call]` generates (`type Op = Self`). The
/// function is written for `W: CallFamily<C = the kind>`; the composition
/// checks that the operation runs on the family it lends ([`RunsOn`]). A
/// kind generic over the structure bounds only what it uses of `K`, such
/// as the shape of its start.
///
/// # What does not compile
///
/// A kind of call that takes its budget from a start of `u64`, on a kind
/// whose start is `()`:
///
/// ```compile_fail,E0271
/// #![feature(impl_trait_in_assoc_type)]
/// use nitori_call::{LentAccess, call};
/// use nitori_driver::{
///     __private::calls::{Count, Item, Root, StillClock, Total},
///     call::{CallFamily, CallKind, Composition, LoanNeeds},
///     component::Part,
///     composition,
///     frunk_core::{HList, hlist, hlist_pat},
///     id::Numbered,
///     need::{Members, Mut, Ref, Under},
///     structure::{NodeKind, Share},
/// };
///
/// /// On the root: the total of every item's count.
/// impl<Q: Composition, K: NodeKind<Start = u64>> CallKind<Q, K> for Counting {
///     type Needs = HList![Mut<Total>, Under<Total, Ref<Count>>];
///     type Op = Self;
///     fn start(_: &mut u64) -> Self {
///         Counting::new()
///     }
/// }
/// #[call]
/// async fn counting<W: CallFamily<C = Counting, K: NodeKind<Start = u64>>>(host: &mut impl LentAccess<Family = W>) {
///     host.with(|mut view| {
///         let hlist_pat![total, items] = view.needs();
///         total.0 = items.iter().map(|(_, c)| c.0).sum();
///     });
/// }
///
/// #[derive(Numbered)]
/// struct Counter;
/// impl<L> Part<L> for Counter {
///     type Args = ();
///     type Shares = HList![
///         Share<Root, HList![Total], HList![Counting]>,
///         Share<Item, HList![Count]>,
///     ];
/// }
/// struct Q;
/// composition!(impl for Q: Root, HList![Counter], StillClock, u8);
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a kind of call at the nodes of the kind `{K}` under `{Q}`",
    label = "a call of a share at this kind",
    note = "a kind of call generic over the structure bounds what it uses of the node kind, such as `K: NodeKind<Start = …>`; a kind that does not meet it cannot have the call"
)]
pub trait CallKind<Q: Composition, K: NodeKind>: 'static + Sized {
    /// The needs, lent at the call's node on every poll.
    type Needs: Need;
    /// The operation the call runs.
    type Op;
    /// The operation of a new node's call. `start` is what the one adding
    /// the node gave it, after the node's components took out what they
    /// keep; the node's calls are handed it in order and may take out what
    /// only they keep.
    fn start(start: &mut K::Start) -> Self::Op;
}

/// The view of the call `C` at a node of the kind `K` under `Q`, its
/// needs at the position `KP`, borrowed for `'v`.
pub type CallView<'v, Q, K, C, KP> = View<'v, <C as CallKind<Q, K>>::Needs, KP>;

/// The family of the call `C` at a node of the kind `K` under `Q`, its
/// needs at the position `KP` the composition proved.
#[allow(clippy::type_complexity)]
pub struct Lent<Q, K, C, KP>(PhantomData<fn() -> (Q, K, C, KP)>);

/// What a call is lent on one poll: the view of its needs, and the
/// driver's side of the call.
pub struct Loan<'v, Q: Composition, K: NodeKind, C: CallKind<Q, K>, KP: Pos> {
    pub(crate) view: CallView<'v, Q, K, C, KP>,
    pub(crate) host: CallHost<PriorityOf<Q>>,
}
impl<Q: Composition, K: NodeKind, C: CallKind<Q, K>, KP: Pos> Loan<'_, Q, K, C, KP> {
    /// From now on, rank the call at `p` when its standard waker, the one
    /// its `.await`s register, is woken. The priority is read when the
    /// wake is handled, so a waker handed out before keeps up with it; it
    /// is the priority the node was added with until set.
    pub fn set_wake_priority(&self, p: PriorityOf<Q>) {
        self.host.set_wake_priority(p);
    }
    /// The wait for the requests the call put in: awaited, outside
    /// `host.with`, it gives way once, the driver handles every request
    /// put in so far, and the call goes on at `p`. The requests are
    /// handled first; after them, what runs next is decided by priority
    /// only. See [`Settle`](super::Settle).
    pub fn settle(&self, p: PriorityOf<Q>) -> super::Settle {
        self.host.settle(p)
    }
}
// A loan holds borrows only; nothing in it is pinned structurally. The
// view's type has projections in it, so `Unpin` is not inferred.
impl<Q: Composition, K: NodeKind, C: CallKind<Q, K>, KP: Pos> Unpin for Loan<'_, Q, K, C, KP> {}

impl<Q: Composition, K: NodeKind, C: CallKind<Q, K>, KP: Pos> ReceiverFamily for Lent<Q, K, C, KP> {
    type ReceiverView<'v>
        = Loan<'v, Q, K, C, KP>
    where
        Self: 'v;
}

/// What a call's `#[call]` function is generic over: the composition, the
/// node kind, the kind of call and the position of its needs.
///
/// The function writes `W: CallFamily<C = the kind>`, and what it asks of
/// the composition as a bound of `Q` there (`CallFamily<C = Receive, Q:
/// DispatchesFrames>`); it names no position.
pub trait CallFamily: ReceiverFamily + 'static {
    /// The composition.
    type Q: Composition;
    /// The kind of the call's node.
    type K: NodeKind;
    /// The kind of the call.
    type C: CallKind<Self::Q, Self::K>;
    /// The position of the call's needs.
    type KP: Pos;
    /// The needs in this poll's view.
    #[allow(clippy::type_complexity)]
    fn needs<'s, 'v>(
        view: Pin<&'s mut Self::ReceiverView<'v>>,
    ) -> &'s mut CallView<'v, Self::Q, Self::K, Self::C, Self::KP>
    where
        Self: 'v;
    /// This poll's loan.
    #[allow(clippy::type_complexity)]
    fn loan<'s, 'v>(
        view: Pin<&'s mut Self::ReceiverView<'v>>,
    ) -> &'s mut Loan<'v, Self::Q, Self::K, Self::C, Self::KP>
    where
        Self: 'v;
}
impl<Q: Composition, K: NodeKind, C: CallKind<Q, K>, KP: Pos> CallFamily for Lent<Q, K, C, KP> {
    type Q = Q;
    type K = K;
    type C = C;
    type KP = KP;
    fn needs<'s, 'v>(view: Pin<&'s mut Loan<'v, Q, K, C, KP>>) -> &'s mut CallView<'v, Q, K, C, KP>
    where
        Self: 'v,
    {
        &mut view.get_mut().view
    }
    fn loan<'s, 'v>(view: Pin<&'s mut Loan<'v, Q, K, C, KP>>) -> &'s mut Loan<'v, Q, K, C, KP>
    where
        Self: 'v,
    {
        view.get_mut()
    }
}

/// The needs of a call's view, as `view.needs()` inside `host.with(|mut
/// view| …)`.
///
/// The view is lent for this poll and stays the loan's; destructuring it
/// gives `&mut` bindings to its items.
pub trait LoanNeeds<'h, W: CallFamily> {
    /// The needs in this poll's view.
    #[allow(clippy::type_complexity)]
    fn needs(&mut self) -> &mut CallView<'h, W::Q, W::K, W::C, W::KP>;
}
impl<'h, W: CallFamily> LoanNeeds<'h, W> for Access<'_, 'h, W> {
    fn needs(&mut self) -> &mut CallView<'h, W::Q, W::K, W::C, W::KP> {
        W::needs(self.as_pin())
    }
}

/// The driver's side of a call's loan, as `view.settle(p)` inside
/// `host.with(|mut view| …)`: [`Loan::set_wake_priority`] and
/// [`Loan::settle`].
pub trait LoanHost<W: CallFamily> {
    /// [`Loan::set_wake_priority`].
    fn set_wake_priority(&mut self, p: PriorityOf<W::Q>);
    /// [`Loan::settle`]: the wait, to await outside `host.with`.
    fn settle(&mut self, p: PriorityOf<W::Q>) -> super::Settle;
}
impl<W: CallFamily> LoanHost<W> for Access<'_, '_, W> {
    fn set_wake_priority(&mut self, p: PriorityOf<W::Q>) {
        W::loan(self.as_pin()).set_wake_priority(p);
    }
    fn settle(&mut self, p: PriorityOf<W::Q>) -> super::Settle {
        W::loan(self.as_pin()).settle(p)
    }
}

/// The operation `Self` runs on the family `F` as a call lent at a node:
/// it returns nothing and yields nothing.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not the operation of a `#[call]` function for the family `{F}`",
    label = "the kind's `Op`",
    note = "a kind's `Op` is the struct its own `#[call]` function generates, the function written for `W: CallFamily<C = the kind>` with what it asks of the composition, returning `()` and yielding nothing"
)]
pub trait RunsOn<F: ReceiverFamily>:
    Operation<F, Output = (), Yield = Infallible> + for<'s> Start<'s, LentSlot<'s, F>>
{
}
impl<F: ReceiverFamily, Op> RunsOn<F> for Op where
    Op: Operation<F, Output = (), Yield = Infallible> + for<'s> Start<'s, LentSlot<'s, F>>
{
}

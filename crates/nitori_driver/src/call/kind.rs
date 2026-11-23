//! The kinds of call and the family they are lent through: a kind's needs
//! and operation, the loan a poll lends, and what a poll comes to.
use core::{marker::PhantomData, pin::Pin};

use frunk_core::hlist::HNil;
use nitori_call::{Access, LentSlot, Operation, ReceiverFamily, Start};

use super::{Composition, PartsOf};
use crate::lend::{Flat, KeyedList, NeedSet, ViewOf};

/// A kind of call lent at nodes, for every composition `Q`: its needs, and
/// the operation of its `#[call]` function.
///
/// The kind is the struct `#[call]` generates (`type Op = Self`), or a
/// marker naming it. The function is written for `W: CallFamily<C = the
/// kind>`; the composition checks that the operation runs on the family it
/// builds ([`RunsOn`]).
///
/// # What does not compile
///
/// A kind whose `Op` is the operation of another kind's function:
///
/// ```compile_fail
/// # #![feature(impl_trait_in_assoc_type, yield_expr)]
/// # use frunk_core::{HList, hlist::HNil, hlist_pat};
/// # use nitori_call::{LentAccess, call};
/// # use nitori_driver::{
/// #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
/// #     composition,
/// #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
/// #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
/// # };
/// # struct Root;
/// # struct Item;
/// # impl NodeKind for Root {
/// #     type Children = HList![Child<Item, SlotVec<u32>>];
/// #     type Linked = HNil;
/// # }
/// # impl NodeKind for Item {
/// #     type Children = HNil;
/// #     type Linked = HNil;
/// # }
/// # #[derive(Default)]
/// # struct Count(u32);
/// # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
/// struct Other;
/// impl<Q: Composition> CallKind<Q> for Other {
///     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
///     type Op = Counting;
///     fn start() -> Counting {
///         Counting::new()
///     }
/// }
/// impl<Q: Composition> CallKind<Q> for Counting {
///     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
///     type Op = Self;
///     fn start() -> Self {
///         Self::new()
///     }
/// }
/// #[call]
/// async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
///     host.with(|mut view| view.needs().head.0 += 1);
/// }
/// struct Part;
/// impl Share<Root> for Part {
///     type Data = HList![Count];
/// }
/// impl Share<Item> for Part {
///     type Data = HList![Count];
/// }
/// impl ShareCalls<Root> for Part {
///     type Calls = HList![Counting, Other];
/// }
/// impl ShareCalls<Item> for Part {
///     type Calls = HList![];
/// }
/// struct Q;
/// composition!(impl for Q: Root, HList![Part]);
/// ```
///
/// A function that yields what its kind does not say it yields:
///
/// ```compile_fail
/// # #![feature(impl_trait_in_assoc_type, yield_expr)]
/// # use frunk_core::{HList, hlist::HNil, hlist_pat};
/// # use nitori_call::{LentAccess, call};
/// # use nitori_driver::{
/// #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
/// #     composition,
/// #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
/// #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
/// # };
/// # struct Root;
/// # struct Item;
/// # impl NodeKind for Root {
/// #     type Children = HList![Child<Item, SlotVec<u32>>];
/// #     type Linked = HNil;
/// # }
/// # impl NodeKind for Item {
/// #     type Children = HNil;
/// #     type Linked = HNil;
/// # }
/// # #[derive(Default)]
/// # struct Count(u32);
/// # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
/// impl<Q: Composition> CallKind<Q> for Counting {
///     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
///     type Op = Self;
///     fn start() -> Self {
///         Self::new()
///     }
/// }
/// #[call(yields = u32)]
/// async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
///     yield 1;
/// }
/// struct Part;
/// impl Share<Root> for Part {
///     type Data = HList![Count];
/// }
/// impl Share<Item> for Part {
///     type Data = HList![Count];
/// }
/// impl ShareCalls<Root> for Part {
///     type Calls = HList![Counting];
/// }
/// impl ShareCalls<Item> for Part {
///     type Calls = HList![];
/// }
/// struct Q;
/// composition!(impl for Q: Root, HList![Part]);
/// ```
pub trait CallKind<Q: Composition>: 'static + Sized {
    /// The needs, an `HList` of needs.
    type Needs: NeedSet<PartsOf<Q>>;
    /// The operation the call runs.
    type Op;
    /// What the call yields to the driver; `Infallible` for a call that
    /// yields nothing.
    type Yield = core::convert::Infallible;
    /// What wakes the call: a list of [`Hear`](crate::event::Hear) and
    /// [`HearAt`](crate::event::HearAt), resolved where the call is lent.
    type Subscribes = HNil;
    /// The operation of a new call.
    fn start() -> Self::Op;
    /// Whether the call is no longer needed, read from the view of its
    /// needs before it is polled; the driver then drops it and does not
    /// poll it again. By default never.
    fn no_longer_needed<KP: Positions<Q, Self>>(view: &View<'_, Self, Q, KP>) -> bool {
        let _ = view;
        false
    }
}
/// The keyed needs of the call `C`, in order.
pub type KeyedOf<Q, C> = <<C as CallKind<Q>>::Needs as Flat>::Keyed;
/// The view of the call `C` under `Q`, its keyed needs at `KP`.
pub type View<'w, C, Q, KP> = ViewOf<'w, <C as CallKind<Q>>::Needs, PartsOf<Q>, KP>;

/// The positions of the keyed needs of the call `C` under `Q`; the view
/// reaches them through this bound only.
pub trait Positions<Q: Composition, C: CallKind<Q>>: KeyedList<PartsOf<Q>, KeyedOf<Q, C>> {}
impl<Q: Composition, C: CallKind<Q>, KP: KeyedList<PartsOf<Q>, KeyedOf<Q, C>>> Positions<Q, C>
    for KP
{
}

/// The family of the call `C` under `Q`, its keyed needs at `KP`.
#[allow(clippy::type_complexity)]
pub struct Lent<Q, C, KP>(PhantomData<fn() -> (Q, C, KP)>);

/// What a call is lent on one poll: the view of its needs.
pub struct Loan<'v, Q: Composition, C: CallKind<Q>, KP: Positions<Q, C>>(
    pub(super) View<'v, C, Q, KP>,
);
// A loan holds borrows only; nothing in it is pinned structurally. The
// view's type has projections in it, so `Unpin` is not inferred.
impl<Q: Composition, C: CallKind<Q>, KP: Positions<Q, C>> Unpin for Loan<'_, Q, C, KP> {}

impl<Q: Composition, C: CallKind<Q>, KP: Positions<Q, C>> ReceiverFamily for Lent<Q, C, KP> {
    type ReceiverView<'v>
        = Loan<'v, Q, C, KP>
    where
        Self: 'v;
}

/// What a call's `#[call]` function is generic over: the composition, the
/// kind, and the keyed positions, whose bound the associated type carries.
pub trait CallFamily: ReceiverFamily + 'static {
    /// The composition.
    type Q: Composition;
    /// The kind of the call.
    type C: CallKind<Self::Q>;
    /// The positions of the call's keyed needs.
    type KP: Positions<Self::Q, Self::C>;
    /// The needs in this poll's view.
    #[allow(clippy::type_complexity)]
    fn needs<'s, 'v>(
        view: Pin<&'s mut Self::ReceiverView<'v>>,
    ) -> &'s mut View<'v, Self::C, Self::Q, Self::KP>
    where
        Self: 'v;
}
impl<Q: Composition, C: CallKind<Q>, KP: Positions<Q, C> + 'static> CallFamily for Lent<Q, C, KP> {
    type Q = Q;
    type C = C;
    type KP = KP;
    fn needs<'s, 'v>(view: Pin<&'s mut Loan<'v, Q, C, KP>>) -> &'s mut View<'v, C, Q, KP>
    where
        Self: 'v,
    {
        &mut view.get_mut().0
    }
}

/// The needs of a call's view, as `view.needs()` inside `host.with(|mut
/// view| …)`.
///
/// The view is lent for this poll and stays the loan's; destructuring it
/// gives `&mut` bindings to its items.
///
/// # What does not compile
///
/// A call's body that takes a keyed view for the component, before any composition:
///
/// ```compile_fail
/// # #![feature(impl_trait_in_assoc_type, yield_expr)]
/// # use frunk_core::{HList, hlist::HNil, hlist_pat};
/// # use nitori_call::{LentAccess, call};
/// # use nitori_driver::{
/// #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
/// #     composition,
/// #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
/// #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
/// # };
/// # struct Root;
/// # struct Item;
/// # impl NodeKind for Root {
/// #     type Children = HList![Child<Item, SlotVec<u32>>];
/// #     type Linked = HNil;
/// # }
/// # impl NodeKind for Item {
/// #     type Children = HNil;
/// #     type Linked = HNil;
/// # }
/// # #[derive(Default)]
/// # struct Count(u32);
/// # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
/// impl<Q: Composition> CallKind<Q> for Counting {
///     type Needs = HList![At<Kind<Root>, ToChild<Item, ByKeyOthers<u32>>, Mut<Count>>];
///     type Op = Self;
///     fn start() -> Self {
///         Self::new()
///     }
/// }
/// #[call]
/// async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
///     host.with(|mut view| {
///         let hlist_pat![others] = view.needs();
///         others.0 = 1;
///     });
/// }
/// ```
pub trait LoanNeeds<'h, W: CallFamily> {
    /// The needs in this poll's view.
    #[allow(clippy::type_complexity)]
    fn needs(&mut self) -> &mut View<'h, W::C, W::Q, W::KP>;
}
impl<'h, W: CallFamily> LoanNeeds<'h, W> for Access<'_, 'h, W> {
    fn needs(&mut self) -> &mut View<'h, W::C, W::Q, W::KP> {
        W::needs(self.as_pin())
    }
}

/// The operation `Self` runs on the family `F` as a call lent at a node,
/// yielding `Y`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not the operation of a `#[call]` function for the family `{F}` that yields `{Y}`",
    label = "the kind's `Op`",
    note = "a kind's `Op` is the struct its own `#[call]` function generates, the function written for `W: CallFamily<C = the kind>`, with `()` as its output and the kind's `Yield` as what it yields"
)]
pub trait RunsOn<F: ReceiverFamily, Y>:
    Operation<F, Output = (), Yield = Y> + for<'s> Start<'s, LentSlot<'s, F>>
{
}
impl<F: ReceiverFamily, Y, Op> RunsOn<F, Y> for Op where
    Op: Operation<F, Output = (), Yield = Y> + for<'s> Start<'s, LentSlot<'s, F>>
{
}

/// What one poll of a call came to.
#[derive(Debug, PartialEq, Eq)]
pub enum CallStep<Y> {
    /// The call waits.
    Pending,
    /// The call is over; it is not polled again.
    Finished,
    /// The call yielded an item and goes on.
    Yielded(Y),
    /// The call said it is no longer needed and was not polled; it is
    /// dropped.
    NoLongerNeeded,
}

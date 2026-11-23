//! Proving every slot: the positions of a call's needs, inferred once into
//! the pack.
//!
//! # What does not compile
//!
//! A call on an item that takes every item by key (`ByKey`), the store its own item is kept in:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type, yield_expr)]
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
//! #     composition,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
//! #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
//! impl<Q: Composition> CallKind<Q> for Counting {
//!     type Needs = HList![At<Kind<Root>, ToChild<Item, ByKey<u32>>, Mut<Count>>];
//!     type Op = Self;
//!     fn start() -> Self {
//!         Self::new()
//!     }
//! }
//! #[call]
//! async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
//!     host.with(|mut view| {
//!         let hlist_pat![items] = view.needs();
//!         let _ = items.get_mut(&1);
//!     });
//! }
//! struct Part;
//! impl Share<Root> for Part {
//!     type Data = HList![Count];
//! }
//! impl Share<Item> for Part {
//!     type Data = HList![Count];
//! }
//! impl ShareCalls<Root> for Part {
//!     type Calls = HList![];
//! }
//! impl ShareCalls<Item> for Part {
//!     type Calls = HList![Counting];
//! }
//! struct Q;
//! composition!(impl for Q: Root, HList![Part]);
//! ```
use alloc::vec::Vec;
use core::{
    any::TypeId,
    marker::PhantomData,
    ops::ControlFlow,
    pin::Pin,
    task::{Context, Poll},
};

use frunk_core::hlist::{HCons, HNil};
use nitori_call::LentCall;

use super::{
    CallKind, CallStep, Composition, KeyedOf, LendInstant, LendOne, Lent, Loan, OutboxOf, PartsOf,
    Positions, RootOf, RunsOn, Slot, SlotCall,
};
use crate::{
    event::{Subscription, Subscriptions},
    lend::{
        Build, Cell, CellsOf, ChainCells, CxOf, Freeze, FrozenOf, KeyedPositions, LendError,
        LinkOf, LinkSpec, LinkedOf, Locate, Pairwise, Pass1, Pass2, Places, PositionsOf, Raw1Of,
        Raw2Of, Rem1Of, Writes,
    },
    need::ChainOf,
    structure::{Node, PathFrom},
};

/// The positions of one slot: the link's, pass 1's, pass 2's, the keyed
/// needs' and the subscriptions'.
#[allow(clippy::type_complexity)]
pub struct SlotPositions<IL, I1, I2, IK, IS>(PhantomData<fn() -> (IL, I1, I2, IK, IS)>);

/// Every slot of a list proved, its positions inferred (`I`, one variable
/// for the whole tree); the pack is the list of the positions.
pub trait Prove<Q, I> {
    /// The pack.
    type Out;
}
impl<Q> Prove<Q, HNil> for HNil {
    type Out = HNil;
}
impl<Q, S, T, IS, IT> Prove<Q, HCons<IS, IT>> for HCons<S, T>
where
    S: ProveOne<Q, IS>,
    T: Prove<Q, IT>,
{
    type Out = HCons<S::Pos, T::Out>;
}
/// The pack of the slots `L` of `Q`; [`composition!`](crate::composition!) defines the opaque
/// pack with it.
pub fn prove<Q: Composition, L: Prove<Q, I>, I>() -> PhantomData<L::Out> {
    PhantomData
}

/// One slot proved: its call can be lent at its nodes under `Q`.
///
/// The positions are inferred at `'static`; they do not depend on the
/// lifetime, and the pack's bound proves them again for every lifetime.
#[diagnostic::on_unimplemented(
    message = "the call in `{Self}` cannot be lent at its node under `{Q}`",
    label = "a need of this call is not met",
    note = "a need's anchor, steps or component is not on this structure, two needs take the same place, a keyed need uses the word that does not fit its store (`ByKey` off the chain, `ByKeyOthers` on it), a part does not contribute to a family the call uses, or the kind's `Op` is not the operation of a `#[call]` function for this kind; `explain::<Q, Kind, Path, _, _, _, _>()` names the need"
)]
pub trait ProveOne<Q, I> {
    /// The slot's positions.
    type Pos;
}

type Needs<Q, C> = <C as CallKind<Q>>::Needs;
/// The chain up from the nodes at the path type `X` of `Q`'s tree.
pub type ChainAt<Q, X> = <X as ChainOf<RootOf<Q>, HNil>>::Chain;
type KPOf<Q, C, X, IK> = PositionsOf<Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IK>;

impl<Q, X, C, IL, IC, IP, IK, I1, I2, IS> ProveOne<Q, (IL, IC, IP, IK, I1, I2, IS)> for Slot<X, C>
where
    Q: Composition,
    C: CallKind<Q>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil>,
    LinkedOf<RootOf<Q>, X>: LinkSpec<RootOf<Q>, PartsOf<Q>, X, IL>,
    X: ChainCells<'static, RootOf<Q>, PartsOf<Q>, HNil, LinkOf<RootOf<Q>, PartsOf<Q>, X, IL>>,
    Needs<Q, C>: Locate<
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            IC,
        >,
    Needs<Q, C>: Places<
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            IP,
        >,
    <Needs<Q, C> as Places<
        CxOf<RootOf<Q>, PartsOf<Q>, X>,
        CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
        IP,
    >>::Out: Pairwise,
    Needs<Q, C>: KeyedPositions<CxOf<RootOf<Q>, PartsOf<Q>, X>, IK>,
    KPOf<Q, C, X, IK>: Positions<Q, C> + 'static,
    Needs<Q, C>: Pass1<
            'static,
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            I1,
        >,
    Rem1Of<'static, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>: Freeze,
    Needs<Q, C>: Pass2<
            'static,
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            FrozenOf<'static, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>,
            I2,
        >,
    C::Op: RunsOn<Lent<Q, C, KPOf<Q, C, X, IK>>, C::Yield>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, PartsOf<Q>, IS>,
    Needs<Q, C>: Writes<PartsOf<Q>>,
{
    type Pos = SlotPositions<IL, I1, I2, IK, IS>;
}

/// The item-by-item check of the call of the kind `C` at the path type `X`
/// under `Q`, each bound on its own, so that the first error names the
/// failing need. It does nothing; call it to read why a composition does
/// not compile.
pub fn explain<Q, C, X, IL, IC, IP, IK>()
where
    Q: Composition,
    C: CallKind<Q>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil>,
    LinkedOf<RootOf<Q>, X>: LinkSpec<RootOf<Q>, PartsOf<Q>, X, IL>,
    X: ChainCells<'static, RootOf<Q>, PartsOf<Q>, HNil, LinkOf<RootOf<Q>, PartsOf<Q>, X, IL>>,
    Needs<Q, C>: Locate<
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            IC,
        >,
    Needs<Q, C>: Places<
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            IP,
        >,
    <Needs<Q, C> as Places<
        CxOf<RootOf<Q>, PartsOf<Q>, X>,
        CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
        IP,
    >>::Out: Pairwise,
    Needs<Q, C>: KeyedPositions<CxOf<RootOf<Q>, PartsOf<Q>, X>, IK>,
{
}

impl<Q, X, C, IL, I1, I2, IK, IS> SlotCall<Q, Slot<X, C>> for SlotPositions<IL, I1, I2, IK, IS>
where
    Q: Composition,
    C: CallKind<Q>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil>,
    Needs<Q, C>: KeyedPositions<CxOf<RootOf<Q>, PartsOf<Q>, X>, IK>,
    KPOf<Q, C, X, IK>: Positions<Q, C> + 'static,
    C::Op: RunsOn<Lent<Q, C, KPOf<Q, C, X, IK>>, C::Yield>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, PartsOf<Q>, IS>,
    Needs<Q, C>: Writes<PartsOf<Q>>,
{
    type Family = Lent<Q, C, KPOf<Q, C, X, IK>>;
    type Running = LentCall<Self::Family, C::Op>;
    fn start(_: &X) -> Self::Running {
        LentCall::new(C::start())
    }
    fn subscriptions(out: &mut Vec<Subscription>) {
        <C::Subscribes as Subscriptions<ChainAt<Q, X>, PartsOf<Q>, IS>>::subscriptions(out);
    }
    fn writes(out: &mut Vec<TypeId>) {
        <Needs<Q, C> as Writes<PartsOf<Q>>>::writes(out);
    }
}

impl<'a, Q, X, C, IL, I1, I2, IK, IS> LendOne<'a, Q, Slot<X, C>>
    for SlotPositions<IL, I1, I2, IK, IS>
where
    Q: Composition,
    C: CallKind<Q>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil>,
    Needs<Q, C>: KeyedPositions<CxOf<RootOf<Q>, PartsOf<Q>, X>, IK>,
    KPOf<Q, C, X, IK>: Positions<Q, C> + 'static,
    C::Op: RunsOn<Lent<Q, C, KPOf<Q, C, X, IK>>, C::Yield>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, PartsOf<Q>, IS>,
    Needs<Q, C>: Writes<PartsOf<Q>>,
    LinkedOf<RootOf<Q>, X>: LinkSpec<RootOf<Q>, PartsOf<Q>, X, IL>,
    X: ChainCells<'a, RootOf<Q>, PartsOf<Q>, HNil, LinkOf<RootOf<Q>, PartsOf<Q>, X, IL>>,
    Needs<Q, C>: Pass1<
            'a,
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'a, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            I1,
        >,
    Rem1Of<'a, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>: Freeze,
    Needs<Q, C>: Pass2<
            'a,
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            FrozenOf<'a, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>,
            I2,
        >,
    Needs<Q, C>: Build<
            'a,
            PartsOf<Q>,
            KeyedOf<Q, C>,
            KPOf<Q, C, X, IK>,
            Raw1Of<'a, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>,
            Raw2Of<'a, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, I2, LendInstant<Q>>,
        >,
{
    fn poll(
        root: &'a mut Node<RootOf<Q>, PartsOf<Q>>,
        at: X,
        outbox: &'a OutboxOf<Q>,
        call: Pin<&mut Self::Running>,
        cx: &mut Context<'_>,
    ) -> Result<CallStep<C::Yield>, LendError> {
        let link =
            <LinkedOf<RootOf<Q>, X> as LinkSpec<RootOf<Q>, PartsOf<Q>, X, IL>>::find(root, &at)?;
        let cells = HCons {
            head: Cell::new(outbox),
            tail: at.cells(root, link, crate::lend::FromSplit)?,
        };
        let (raw1, rem) =
            <Needs<Q, C> as Pass1<'a, CxOf<RootOf<Q>, PartsOf<Q>, X>, _, I1>>::take(cells);
        let frozen = rem.freeze();
        let raw2 = <Needs<Q, C> as Pass2<'a, CxOf<RootOf<Q>, PartsOf<Q>, X>, _, I2>>::get(&frozen);
        let mut loan: Loan<'a, Q, C, KPOf<Q, C, X, IK>> = Loan(<Needs<Q, C> as Build<
            'a,
            PartsOf<Q>,
            KeyedOf<Q, C>,
            KPOf<Q, C, X, IK>,
            _,
            _,
        >>::view(raw1, raw2));
        if C::no_longer_needed::<KPOf<Q, C, X, IK>>(&loan.0) {
            return Ok(CallStep::NoLongerNeeded);
        }
        Ok(match call.poll_with_views(Pin::new(&mut loan), cx) {
            Poll::Pending => CallStep::Pending,
            Poll::Ready(ControlFlow::Break(())) => CallStep::Finished,
            Poll::Ready(ControlFlow::Continue(item)) => CallStep::Yielded(item),
        })
    }
}

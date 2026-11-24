//! A composition: its root kind, its parts, its clock and priority, the
//! context its lendings take, the pack of what it proved, and the macro
//! that writes one.
use alloc::vec::Vec;
use core::marker::PhantomData;

#[cfg(test)]
use frunk_core::hlist::Selector;
use frunk_core::hlist::{HCons, HNil};
use nitori_clock::MonotonicClock;

use super::{
    kinds::{AllKinds, RequestLists},
    pack::{KindDesc, KindList, PackList},
};
use crate::{
    component::{ArgsOf, PartList},
    lend::{
        context::{Context, WakeList},
        entry::{BuildRootShares, build_root},
    },
    need::LentClock,
    structure::{ChildAddresses, KindFor, Node, NodeKind},
};
#[cfg(test)]
use crate::{lend::context::RequestList, request::NodeRequest};

/// A composition: a root kind, a list of parts, the driver's clock and
/// the priority a waking has, and what was proved once of every kind of
/// the structure.
///
/// Written with [`composition!`](crate::composition!); the pack is opaque,
/// defined by [`pack`](Composition::pack).
pub trait Composition: 'static + Sized {
    /// The root kind: the first of its kinds, whose nodes have no key.
    type Root: KindFor<Self::Parts, Children: ChildAddresses<Self::Parts>>
        + AllKinds<
            Kinds: KindList<Self, Head: KindDesc<Self, Kind = Self::Root, Fk = HNil>>
                       + RequestLists<Self::Root>,
        >;
    /// The list of parts.
    type Parts: PartList<Self::Parts> + 'static;
    /// The clock the driver reads for every lending; a
    /// [`Clock`](crate::need::Clock)`<C>` of another clock does not
    /// compile.
    type Clock: MonotonicClock + 'static;
    /// What a waking is ranked by, the highest handled first; its levels
    /// are the user's.
    type Priority: Ord + Copy + 'static;
    /// What was proved of every kind of the structure, in the order of
    /// [`KindsOf`].
    type Pack: PackList<Self, KindsOf<Self>>;
    /// Only to define `Pack`.
    fn pack() -> PhantomData<Self::Pack>;
    /// The root of a new tree: every component of the root built in the
    /// order of the parts, from `start` and the arguments.
    fn root(
        start: &mut <Self::Root as NodeKind>::Start,
        args: &ArgsOf<Self::Parts>,
    ) -> RootNode<Self>;
}
/// The root kind of `Q`.
pub type RootOf<Q> = <Q as Composition>::Root;
/// The list of parts of `Q`.
pub type PartsOf<Q> = <Q as Composition>::Parts;
/// The clock of `Q`.
pub type ClockOf<Q> = <Q as Composition>::Clock;
/// The priority of `Q`.
pub type PriorityOf<Q> = <Q as Composition>::Priority;
/// Every kind of `Q`'s structure, the root first, each before the kinds
/// below it.
pub type KindsOf<Q> = <RootOf<Q> as AllKinds>::Kinds;
/// The root node of `Q`'s tree.
pub type RootNode<Q> = Node<RootOf<Q>, PartsOf<Q>>;
/// The context every lending under `Q` takes: the instant the driver read
/// from its clock, the wakes at `Q`'s priority, then a request box for
/// every kind of the structure, in the order of [`KindsOf`].
pub type ContextOf<Q> = Context<
    HCons<
        LentClock<ClockOf<Q>>,
        HCons<WakeList<PriorityOf<Q>>, <KindsOf<Q> as RequestLists<RootOf<Q>>>::Lists>,
    >,
>;

/// The root of a new tree of the root kind `R` for the parts `Pt`, which
/// [`composition!`](crate::composition!) defines [`Composition::root`]
/// with.
#[doc(hidden)]
#[allow(private_bounds)]
pub fn new_root<R, Pt>(start: &mut R::Start, args: &ArgsOf<Pt>) -> Node<R, Pt>
where
    R: KindFor<Pt>,
    Pt: PartList<Pt> + BuildRootShares<R, Pt, Data = <R as KindFor<Pt>>::Data>,
{
    Node::new(build_root::<R, Pt>(start, args))
}

/// The context of a lending under `Q`: `clock` read at `now`, no wakes,
/// and every request box empty.
pub(crate) fn context<Q: Composition>(
    clock: ClockOf<Q>,
    now: <ClockOf<Q> as MonotonicClock>::Instant,
) -> ContextOf<Q> {
    Context::new(HCons {
        head: LentClock::new(clock, now),
        tail: HCons {
            head: WakeList::new(),
            tail: <KindsOf<Q> as RequestLists<RootOf<Q>>>::lists(),
        },
    })
}

/// The requests for the nodes of the kind `K` put in the lendings of
/// `cx`, each with its place in the order of the context, leaving the box
/// empty; `I` is inferred.
#[cfg(test)]
pub(crate) fn take_requests<Q, K, I>(cx: &mut ContextOf<Q>) -> Vec<(u64, NodeRequest<RootOf<Q>, K>)>
where
    Q: Composition,
    K: NodeKind,
    <KindsOf<Q> as RequestLists<RootOf<Q>>>::Lists:
        Selector<RequestList<NodeRequest<RootOf<Q>, K>>, I>,
{
    cx.pieces_mut().tail.tail.get_mut().take()
}

/// The request lists of every kind of `Q`.
pub(crate) type RequestListsOf<Q> = <KindsOf<Q> as RequestLists<RootOf<Q>>>::Lists;

/// The request boxes of every kind of `Q` in `cx`.
pub(crate) fn request_lists<Q: Composition>(cx: &mut ContextOf<Q>) -> &mut RequestListsOf<Q> {
    &mut cx.pieces_mut().tail.tail
}

/// The wakes put in the lendings of `cx`, in order, leaving the list
/// empty.
pub(crate) fn take_wakes<Q: Composition>(
    cx: &mut ContextOf<Q>,
) -> Vec<(crate::need::ComponentWaker, PriorityOf<Q>)> {
    cx.pieces_mut().tail.head.take()
}

/// The wakes put in the lendings of `cx`, in order, swapped with those
/// in `buf`: `buf`, empty, keeps its room for the lendings after.
pub(crate) fn swap_wakes<Q: Composition>(
    cx: &mut ContextOf<Q>,
    buf: &mut Vec<(crate::need::ComponentWaker, PriorityOf<Q>)>,
) {
    cx.pieces_mut().tail.head.swap(buf);
}

/// Read the clock of `cx` again, at `now`.
pub(crate) fn set_now<Q: Composition>(
    cx: &mut ContextOf<Q>,
    now: <ClockOf<Q> as MonotonicClock>::Instant,
) {
    cx.pieces_mut().head.set_now(now);
}

/// Write a composition: `composition!(impl for Q: Root, Parts, Clock,
/// Priority)`.
///
/// The calls come from the parts' shares ([`Share::Calls`](crate::structure::Share::Calls));
/// every call, every component's operations, every kind's build and the
/// driver's reaction points are proved where it is written, once, and
/// what the compiler infers goes into the opaque
/// [`Composition::Pack`]. The crate that writes it enables the nightly
/// feature `impl_trait_in_assoc_type`. A kind at two places of the
/// structure is refused, with an error that names the kind.
///
/// # What does not compile
///
/// A call whose needs overlap, both taking the root's total, one of them
/// exclusive:
///
/// ```compile_fail,E0277
/// #![feature(impl_trait_in_assoc_type)]
/// use nitori_call::{LentAccess, call};
/// use nitori_driver::{
///     __private::calls::{Count, Item, Root, StillClock, Total},
///     call::{CallFamily, CallKind, Composition, LoanNeeds},
///     component::Part,
///     composition,
///     frunk_core::{HList, hlist_pat},
///     need::{Mut, Ref},
///     structure::{NodeKind, Share},
/// };
///
/// /// On the root: the total of every item's count.
/// impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Counting {
///     type Needs = HList![Mut<Total>, Ref<Total>];
///     type Op = Self;
///     fn start(_: &mut K::Start) -> Self {
///         Counting::new()
///     }
/// }
/// #[call]
/// async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
///     host.with(|mut view| {
///         let hlist_pat![total, _] = view.needs();
///         total.0 += 1;
///     });
/// }
///
/// struct Counter;
/// impl<L> Part<L> for Counter {
///     type Args = ();
/// }
/// impl Share<Root> for Counter {
///     type Data = HList![Total];
///     type Calls = HList![Counting];
/// }
/// impl Share<Item> for Counter {
///     type Data = HList![Count];
/// }
/// struct Q;
/// composition!(impl for Q: Root, HList![Counter], StillClock, u8);
/// ```
///
/// A kind at two places, below the left and below the right:
///
/// ```compile_fail,E0277
/// #![feature(impl_trait_in_assoc_type)]
/// use nitori_driver::{
///     __private::calls::{Count, StillClock, Total},
///     component::Part,
///     composition,
///     frunk_core::HList,
///     id::Numbered,
///     structure::{Child, NodeKind, Share, SlotVec},
/// };
/// #[derive(Numbered)]
/// struct Top;
/// impl NodeKind for Top {
///     type Children = HList![Child<Left, SlotVec<u32>>, Child<Right, SlotVec<u32>>];
/// }
/// #[derive(Numbered)]
/// struct Left;
/// impl NodeKind for Left {
///     type Children = HList![Child<Leaf, SlotVec<u32>>];
/// }
/// #[derive(Numbered)]
/// struct Right;
/// impl NodeKind for Right {
///     type Children = HList![Child<Leaf, SlotVec<u32>>];
/// }
/// /// Below the left and below the right.
/// #[derive(Numbered)]
/// struct Leaf;
/// impl NodeKind for Leaf {
///     type Children = HList![];
/// }
/// struct One;
/// impl<L> Part<L> for One {
///     type Args = ();
/// }
/// impl Share<Top> for One {
///     type Data = HList![Total];
/// }
/// impl Share<Left> for One {
///     type Data = HList![];
/// }
/// impl Share<Right> for One {
///     type Data = HList![];
/// }
/// impl Share<Leaf> for One {
///     type Data = HList![Count];
/// }
/// struct Q;
/// composition!(impl for Q: Top, HList![One], StillClock, u8);
/// ```
///
/// Written out by hand, the composition is refused the same way, by the
/// proof of the pack:
///
/// ```compile_fail,E0277
/// # #![feature(impl_trait_in_assoc_type)]
/// # use nitori_driver::{
/// #     __private::calls::{Count, StillClock, Total},
/// #     component::Part,
/// #     composition,
/// #     frunk_core::HList,
/// #     id::Numbered,
/// #     structure::{Child, NodeKind, Share, SlotVec},
/// # };
/// # #[derive(Numbered)]
/// # struct Top;
/// # impl NodeKind for Top {
/// #     type Children = HList![Child<Left, SlotVec<u32>>, Child<Right, SlotVec<u32>>];
/// # }
/// # #[derive(Numbered)]
/// # struct Left;
/// # impl NodeKind for Left {
/// #     type Children = HList![Child<Leaf, SlotVec<u32>>];
/// # }
/// # #[derive(Numbered)]
/// # struct Right;
/// # impl NodeKind for Right {
/// #     type Children = HList![Child<Leaf, SlotVec<u32>>];
/// # }
/// # /// Below the left and below the right.
/// # #[derive(Numbered)]
/// # struct Leaf;
/// # impl NodeKind for Leaf {
/// #     type Children = HList![];
/// # }
/// # struct One;
/// # impl<L> Part<L> for One {
/// #     type Args = ();
/// # }
/// # impl Share<Top> for One {
/// #     type Data = HList![Total];
/// # }
/// # impl Share<Left> for One {
/// #     type Data = HList![];
/// # }
/// # impl Share<Right> for One {
/// #     type Data = HList![];
/// # }
/// # impl Share<Leaf> for One {
/// #     type Data = HList![Count];
/// # }
/// use nitori_driver::call::{
///     Composition, KindsOf, RootNode,
///     __pack::{PackList, new_root, prove},
/// };
/// struct Q;
/// impl Composition for Q {
///     type Root = Top;
///     type Parts = HList![One];
///     type Clock = StillClock;
///     type Priority = u8;
///     type Pack = impl PackList<Self, KindsOf<Self>>;
///     fn pack() -> core::marker::PhantomData<Self::Pack> {
///         prove::<Self, KindsOf<Self>, _>()
///     }
///     fn root(
///         start: &mut <Top as NodeKind>::Start,
///         args: &nitori_driver::component::ArgsOf<HList![One]>,
///     ) -> RootNode<Self> {
///         new_root::<Top, HList![One]>(start, args)
///     }
/// }
/// ```
#[macro_export]
macro_rules! composition {
    (impl for $q:ty: $root:ty, $parts:ty, $clock:ty, $priority:ty $(,)?) => {
        impl $crate::call::Composition for $q {
            type Root = $root;
            type Parts = $parts;
            type Clock = $clock;
            type Priority = $priority;
            type Pack = impl $crate::call::__pack::PackList<Self, $crate::call::KindsOf<Self>>;
            fn pack() -> ::core::marker::PhantomData<Self::Pack> {
                $crate::call::__pack::prove::<Self, $crate::call::KindsOf<Self>, _>()
            }
            fn root(
                start: &mut <$root as $crate::structure::NodeKind>::Start,
                args: &$crate::component::ArgsOf<$parts>,
            ) -> $crate::call::RootNode<Self> {
                $crate::call::__pack::new_root::<$root, $parts>(start, args)
            }
        }
    };
}

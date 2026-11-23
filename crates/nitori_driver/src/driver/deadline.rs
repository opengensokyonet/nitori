//! The deadlines of the components: their timers, reading them again and
//! checking them.
//!
//! # What does not compile
//!
//! A deadline whose instant is not the clock's:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Now, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>, _: &mut ()) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! #[derive(Default)]
//! struct Due;
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Layer> for Due {
//!     type Timing = HasDeadline;
//!     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Layer>>>(
//!         _: BuildView<'_, Self, Part1, L, Layer, KP>,
//!         _: &ArgsOf<L>,
//!         _: &mut (),
//!     ) -> Self {
//!         Due
//!     }
//! }
//! impl<L: PartList<L> + 'static> Timed<Part1, L, Layer> for Due {
//!     type Instant = u64;
//!     fn deadline<KP: HookKeys<Self, Part1, L, Layer>>(
//!         _: &HookView<'_, Self, Part1, L, Layer, KP>,
//!     ) -> Option<u64> {
//!         None
//!     }
//!     fn expire<KP: HookKeys<Self, Part1, L, Layer>>(
//!         _: HookView<'_, Self, Part1, L, Layer, KP>,
//!         _: u64,
//!     ) -> Option<Self::Returns> {
//!         None
//!     }
//! }
//! impl Share<Layer> for Part1 {
//!     type Data = HList![Due];
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
//!
//! A need of the instant that is not the clock's:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Now, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>, _: &mut ()) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! #[derive(Default)]
//! struct Reads;
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Layer> for Reads {
//!     type Needs = HList![At<Up<0>, Here, Now<u64>>];
//!     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Layer>>>(
//!         _: BuildView<'_, Self, Part1, L, Layer, KP>,
//!         _: &ArgsOf<L>,
//!         _: &mut (),
//!     ) -> Self {
//!         Reads
//!     }
//! }
//! impl Share<Layer> for Part1 {
//!     type Data = HList![Reads];
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
use alloc::vec::Vec;

use frunk_core::indices::Here as I0;

use super::{
    AddressOf, Driver, DriverComposition, InstantOf, NthPlace, Place, PlaceDesc, PlaceNth,
    PlaceOps, PlacesOf, RootNode,
    walk::{CallWalk, Cursor, PlaceVisit, PlaceWalk},
};
use crate::{
    call::{OutboxOf, PartsOf, RootOf},
    structure::{ChildAddresses, End, NodeKind, RemainsOf},
};

struct Deadline<'r, Q: DriverComposition> {
    root: &'r mut RootNode<Q>,
    at: &'r AddressOf<Q>,
    c: usize,
    outbox: &'r OutboxOf<Q>,
}
impl<Q: DriverComposition> PlaceVisit<Q> for Deadline<'_, Q> {
    type Out = Option<InstantOf<Q>>;
    fn visit<Pl: PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out {
        Pos::deadline(self.root, self.at, self.c, self.outbox)
    }
}

impl<Q: DriverComposition> Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
{
    pub(super) fn set_deadline(&mut self, id: usize, c: usize, at: Option<InstantOf<Q>>) {
        if let Some(old) = self.deadlines.remove(&(id, c)) {
            self.timers.remove(&(old, id, c));
        }
        if let Some(at) = at {
            self.deadlines.insert((id, c), at);
            self.timers.insert((at, id, c));
        }
    }
    pub(super) fn deadline(&mut self, id: usize, c: usize) -> Option<InstantOf<Q>> {
        let n = self.live(id)?;
        let (p, addr) = (n.place, n.addr);
        let root = self.root.as_mut()?;
        let at = <Cursor<I0> as PlaceWalk<Q>>::visit(
            p,
            Deadline::<Q> {
                root,
                at: &addr,
                c,
                outbox: &self.outbox,
            },
        )
        .flatten();
        // reading a deadline publishes nothing
        let _ = self.outbox.take();
        at
    }
    pub(super) fn read_deadline(&mut self, id: usize, c: usize) {
        let at = self.deadline(id, c);
        self.set_deadline(id, c, at);
    }
    #[cfg(debug_assertions)]
    pub(super) fn check_deadlines(&mut self) {
        for id in self.live_ids().collect::<Vec<_>>() {
            let p = self.nodes[id].live().map_or(0, |n| n.place);
            for c in 0..self.places[p].info.comps.len() {
                if !self.places[p].info.comps[c].timed {
                    continue;
                }
                let at = self.deadline(id, c);
                debug_assert_eq!(
                    self.deadlines.get(&(id, c)).copied(),
                    at,
                    "a deadline changed and was not read again: a lend wrote what it depends on without taking the component exclusive, or the component does not hear an event that moves it",
                );
            }
        }
    }
}

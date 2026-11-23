//! The cases the review of the driver found: a move that fails, replacing
//! a node others link to, a build that takes a parent's component with a
//! deadline exclusive, an event heard through two subscriptions, a
//! deadline that does not move after it expired, and why an add fails.
#![feature(impl_trait_in_assoc_type)]
mod common;

use core::{pin::Pin, task::Poll, time::Duration};
use std::{
    cell::{Cell, RefCell},
    task::{Context, Waker},
};

use common::clock::TestClock;
use frunk_core::{HList, hlist::HNil, hlist_pat};
use nitori_driver::{
    call::ShareCalls,
    composition,
    driver::{AddError, Driver},
    event::{Hear, HearAt, NodeStarted, Triggered},
    hooks::{
        ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList,
        Timed,
    },
    lend::KeyedList,
    need::{At, Here, Kind, Mut, ToChild, Up},
    request::{
        Edit, Editor, EndError, LinkedToError, NoChildError, NodeRequest, ReplaceError, Request,
    },
    structure::{
        AtMostOne, ByOwnKey, Child, End, KeyAddress, LinkedStart, MoveError, NoParentError,
        NodeKind, NodePath, OccupiedError, Share, SlotVec, Store,
    },
};

fn poll<Q: nitori_driver::driver::DriverComposition>(d: &mut Driver<Q>) -> Poll<()>
where
    Driver<Q>: core::future::Future<Output = ()>,
    nitori_driver::__private::Cursor<frunk_core::indices::Here>:
        nitori_driver::__private::CallWalk<Q>,
{
    Pin::new(d).poll(&mut Context::from_waker(Waker::noop()))
}

pub struct Base;
impl<L> Part<L> for Base {
    type Args = ();
}

/// A root that asks itself what the test queues, before its calls.
pub struct Asker<R: 'static>(core::marker::PhantomData<fn() -> R>);
thread_local! {
    static WAKER: RefCell<Option<Waker>> = const { RefCell::new(None) };
}
fn wake() {
    if let Some(w) = WAKER.with(|w| w.borrow_mut().take()) {
        w.wake();
    }
}

// ---------------------------------------------------------------- moves

mod moves {
    use super::*;

    pub struct Root;
    pub struct A;
    pub struct B;
    impl NodeKind for Root {
        type Children = HList![Child<A, SlotVec<u32>>];
        type Linked = HNil;
        type Request = MoveB;
    }
    impl NodeKind for A {
        type Children = HList![Child<B, SlotVec<u32>>];
        type Linked = HNil;
    }
    impl NodeKind for B {
        type Children = HNil;
        type Linked = HNil;
    }
    pub struct MoveB {
        pub from: u32,
        pub key: u32,
        pub to: u32,
    }
    thread_local! {
        pub static RESULT: RefCell<Option<Result<(), MoveError>>> = const { RefCell::new(None) };
        pub static QUEUE: RefCell<Vec<MoveB>> = const { RefCell::new(Vec::new()) };
    }
    impl Request<Root> for MoveB {
        fn apply<E: Editor<Root>>(self, mut edit: Edit<'_, E, Root>) {
            let r = edit.move_grandchild::<A, _, B, _>(&self.from, &self.key, &self.to);
            RESULT.with(|x| *x.borrow_mut() = Some(r));
        }
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<A> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<B> for Base {
        type Calls = HNil;
    }
    impl Share<A> for Base {
        type Data = HNil;
    }
    impl Share<B> for Base {
        type Data = HNil;
    }
    impl Share<Root> for Base {
        type Data = HList![Asker<Root>];
    }
    impl<L: PartList<L> + 'static> Hooks<Base, L, Root> for Asker<Root> {
        const ACTS_BEFORE_CALLS: bool = true;
        type Returns = NodeRequest<Root, Root>;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Root>>>(
            _: BuildView<'_, Self, Base, L, Root, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Asker(core::marker::PhantomData)
        }
        fn before_calls<KP: HookKeys<Self, Base, L, Root>>(
            _: HookView<'_, Self, Base, L, Root, KP>,
            w: &Waker,
        ) -> Option<NodeRequest<Root, Root>> {
            WAKER.with(|x| *x.borrow_mut() = Some(w.clone()));
            QUEUE
                .with(|q| q.borrow_mut().pop())
                .map(|m| NodeRequest::new(KeyAddress::here(), m))
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
}

/// A move refused because the destination keeps the key leaves the node
/// where it was, under its handle: the driver still reaches it.
#[test]
fn a_failed_move_leaves_the_node_where_it_was() {
    use moves::*;
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    let _ = poll(&mut d);
    let a1 = d.add::<A, _, _, _>(End, 1, ()).unwrap();
    let a2 = d.add::<A, _, _, _>(End, 2, ()).unwrap();
    let b10 = d.add::<B, _, _, _>(a1, 10, ()).unwrap();
    let b11 = d.add::<B, _, _, _>(a1, 11, ()).unwrap();
    d.add::<B, _, _, _>(a2, 11, ()).unwrap();
    assert!(d.remove(b10, ()));
    QUEUE.with(|q| {
        q.borrow_mut().push(MoveB {
            from: 1,
            key: 11,
            to: 2,
        })
    });
    wake();
    let _ = poll(&mut d);
    assert_eq!(
        RESULT.with(|r| r.borrow_mut().take()),
        Some(Err(MoveError::Occupied {
            source: OccupiedError
        }))
    );
    let a1n = a1.node(d.root().unwrap()).unwrap();
    assert_eq!(a1n.children.head.key(1), Some(&11));
    assert_eq!(a1n.children.head.key(0), None);
    assert!(d.remove(b11, ()));
    assert!(b11.node(d.root().unwrap()).is_none());
    // a move to a node not there leaves it too
    let b12 = d.add::<B, _, _, _>(a1, 12, ()).unwrap();
    QUEUE.with(|q| {
        q.borrow_mut().push(MoveB {
            from: 1,
            key: 12,
            to: 3,
        })
    });
    wake();
    let _ = poll(&mut d);
    assert_eq!(
        RESULT.with(|r| r.borrow_mut().take()),
        Some(Err(MoveError::NoDestination))
    );
    assert!(b12.node(d.root().unwrap()).is_some());
}

// ---------------------------------------------------------------- replacing a linked node

mod linked {
    use super::*;

    pub struct Root;
    pub struct Entry;
    pub struct Path;
    pub struct Other;
    impl NodeKind for Root {
        type Children = HList![
            Child<Entry, SlotVec<u32>>,
            Child<Path, SlotVec<u32>>,
            Child<Other, SlotVec<u32>>,
        ];
        type Linked = HNil;
        type Request = ReplaceEntry;
    }
    impl NodeKind for Entry {
        type Children = HNil;
        type Linked = HNil;
    }
    impl NodeKind for Path {
        type Children = HNil;
        type Linked = HList![ByOwnKey<Entry>];
    }
    impl LinkedStart<Entry> for Path {
        fn linked_start(_: &mut ()) {}
    }
    impl NodeKind for Other {
        type Children = HNil;
        type Linked = HNil;
    }
    pub struct ReplaceEntry(pub u32);
    thread_local! {
        pub static QUEUE: RefCell<Vec<ReplaceEntry>> = const { RefCell::new(Vec::new()) };
        pub static REPLACED: RefCell<Option<Result<(), ReplaceError>>> = const { RefCell::new(None) };
    }
    impl Request<Root> for ReplaceEntry {
        fn apply<E: Editor<Root>>(self, mut edit: Edit<'_, E, Root>) {
            REPLACED.with(|r| *r.borrow_mut() = Some(edit.replace::<Entry, _>(self.0, (), ())));
        }
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Entry> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Path> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Other> for Base {
        type Calls = HNil;
    }
    impl Share<Entry> for Base {
        type Data = HNil;
    }
    impl Share<Path> for Base {
        type Data = HNil;
    }
    impl Share<Other> for Base {
        type Data = HNil;
    }
    impl Share<Root> for Base {
        type Data = HList![Asker<Root>];
    }
    impl<L: PartList<L> + 'static> Hooks<Base, L, Root> for Asker<Root> {
        const ACTS_BEFORE_CALLS: bool = true;
        type Returns = NodeRequest<Root, Root>;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Root>>>(
            _: BuildView<'_, Self, Base, L, Root, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Asker(core::marker::PhantomData)
        }
        fn before_calls<KP: HookKeys<Self, Base, L, Root>>(
            _: HookView<'_, Self, Base, L, Root, KP>,
            w: &Waker,
        ) -> Option<NodeRequest<Root, Root>> {
            WAKER.with(|x| *x.borrow_mut() = Some(w.clone()));
            QUEUE
                .with(|q| q.borrow_mut().pop())
                .map(|m| NodeRequest::new(KeyAddress::here(), m))
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
}

/// A node others link to is not replaced, as it is not ended; it ends
/// after the last node linking to it, and a node added later is not ended
/// in its place.
#[test]
fn a_linked_node_is_not_replaced() {
    use linked::*;
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    let _ = poll(&mut d);
    let o9 = d.add::<Other, _, _, _>(End, 9, ()).unwrap();
    let p1 = d.add::<Path, _, _, _>(End, 1, ()).unwrap();
    assert!(d.remove(o9, ()));
    QUEUE.with(|q| q.borrow_mut().push(ReplaceEntry(1)));
    wake();
    let _ = poll(&mut d);
    assert_eq!(
        REPLACED.with(|r| r.borrow_mut().take()),
        Some(Err(ReplaceError::End {
            source: EndError::LinkedTo {
                source: LinkedToError
            }
        }))
    );
    QUEUE.with(|q| q.borrow_mut().push(ReplaceEntry(5)));
    wake();
    let _ = poll(&mut d);
    assert_eq!(
        REPLACED.with(|r| r.borrow_mut().take()),
        Some(Err(ReplaceError::End {
            source: EndError::NoChild {
                source: NoChildError
            }
        }))
    );
    d.add::<Other, _, _, _>(End, 8, ()).unwrap();
    assert!(d.remove(p1, ()));
    let r = d.root().unwrap();
    assert!(r.children.head.find(&1).is_none());
    assert!(r.children.tail.tail.head.find(&8).is_some());
}

// ---------------------------------------------------------------- a build that writes a parent's deadline

mod build_writes {
    use super::*;

    pub struct Root;
    pub struct Layer;
    impl NodeKind for Root {
        type Children = HList![Child<Layer, AtMostOne>];
        type Linked = HNil;
    }
    impl NodeKind for Layer {
        type Children = HNil;
        type Linked = HNil;
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Layer> for Base {
        type Calls = HNil;
    }
    impl Share<Root> for Base {
        type Data = HList![Count];
    }
    impl Share<Layer> for Base {
        type Data = HList![Setter];
    }
    pub struct Count(pub u64);
    impl<L: PartList<L> + 'static> Hooks<Base, L, Root> for Count {
        type Timing = HasDeadline;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Root>>>(
            _: BuildView<'_, Self, Base, L, Root, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Count(5)
        }
    }
    impl<L: PartList<L> + 'static> Timed<Base, L, Root> for Count {
        type Instant = Duration;
        fn deadline<KP: HookKeys<Self, Base, L, Root>>(
            v: &HookView<'_, Self, Base, L, Root, KP>,
        ) -> Option<Duration> {
            Some(Duration::from_secs(v.head.0))
        }
        fn expire<KP: HookKeys<Self, Base, L, Root>>(
            v: HookView<'_, Self, Base, L, Root, KP>,
            _: Duration,
        ) -> Option<Self::Returns> {
            v.head.0 += 1000;
            None
        }
    }
    /// Takes the root's count exclusive when it is built, and moves its
    /// deadline.
    pub struct Setter;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Layer> for Setter {
        type BuildNeeds = HList![At<Up<0>, Here, Mut<Count>>];
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Layer>>>(
            p: BuildView<'_, Self, Base, L, Layer, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            let hlist_pat![count] = p;
            count.0 = 100;
            Setter
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
}

/// The deadline of a component a build took exclusive at the parent is
/// read again: it does not come at the old instant, and debug builds find
/// nothing stale.
#[test]
fn a_build_that_writes_a_parent_s_deadline() {
    use build_writes::*;
    let clock = TestClock::default();
    let mut d = Driver::<Q>::new(clock.clone(), frunk_core::hlist![()], ()).unwrap();
    let _ = poll(&mut d);
    d.add::<Layer, _, _, _>(End, (), ()).unwrap();
    clock.set(Duration::from_secs(50));
    let _ = poll(&mut d);
    assert_eq!(d.root().unwrap().data.head.head.0, 100);
    clock.set(Duration::from_secs(100));
    let _ = poll(&mut d);
    assert_eq!(d.root().unwrap().data.head.head.0, 1100);
}

// ---------------------------------------------------------------- one event, two subscriptions

mod twice {
    use super::*;

    pub struct Root;
    pub struct Left;
    pub struct Right;
    impl NodeKind for Root {
        type Children = HList![Child<Left, AtMostOne>, Child<Right, AtMostOne>];
        type Linked = HNil;
    }
    impl NodeKind for Left {
        type Children = HNil;
        type Linked = HNil;
    }
    impl NodeKind for Right {
        type Children = HNil;
        type Linked = HNil;
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Left> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Right> for Base {
        type Calls = HNil;
    }
    impl Share<Root> for Base {
        type Data = HNil;
    }
    impl Share<Right> for Base {
        type Data = HNil;
    }
    impl Share<Left> for Base {
        type Data = HList![Listener];
    }
    thread_local! {
        pub static SAW: Cell<(u32, bool)> = const { Cell::new((0, false)) };
    }
    pub struct Listener;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Left> for Listener {
        type Subscribes = HList![
            Hear<NodeStarted<Right>>,
            HearAt<NodeStarted<Right>, Kind<Root>, ToChild<Right>>,
        ];
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Left>>>(
            _: BuildView<'_, Self, Base, L, Left, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Listener
        }
        fn on_events<KP: HookKeys<Self, Base, L, Left>>(
            e: Triggered<'_>,
            _: HookView<'_, Self, Base, L, Left, KP>,
        ) -> Option<Self::Returns> {
            SAW.with(|s| s.set((s.get().0 + 1, e.contains::<NodeStarted<Right>>())));
            None
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
}

/// An event that reaches a handler through its second subscription only
/// is among the events it is told.
#[test]
fn an_event_heard_through_the_second_subscription() {
    use twice::*;
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    let _ = poll(&mut d);
    d.add::<Left, _, _, _>(End, (), ()).unwrap();
    d.add::<Right, _, _, _>(End, (), ()).unwrap();
    assert_eq!(SAW.with(Cell::get), (1, true));
}

// ---------------------------------------------------------------- a deadline that does not move

mod stuck {
    use super::*;

    pub struct Root;
    impl NodeKind for Root {
        type Children = HNil;
        type Linked = HNil;
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl Share<Root> for Base {
        type Data = HList![Stuck];
    }
    thread_local! {
        pub static EXPIRED: Cell<u32> = const { Cell::new(0) };
    }
    /// Its deadline stays at 1 s after it expires: a component in error.
    pub struct Stuck;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Root> for Stuck {
        type Timing = HasDeadline;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Root>>>(
            _: BuildView<'_, Self, Base, L, Root, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Stuck
        }
    }
    impl<L: PartList<L> + 'static> Timed<Base, L, Root> for Stuck {
        type Instant = Duration;
        fn deadline<KP: HookKeys<Self, Base, L, Root>>(
            _: &HookView<'_, Self, Base, L, Root, KP>,
        ) -> Option<Duration> {
            Some(Duration::from_secs(1))
        }
        fn expire<KP: HookKeys<Self, Base, L, Root>>(
            _: HookView<'_, Self, Base, L, Root, KP>,
            _: Duration,
        ) -> Option<Self::Returns> {
            EXPIRED.with(|e| e.set(e.get() + 1));
            None
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
}

/// Debug builds say that a deadline did not move after it expired.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "after its expiry a deadline is later")]
fn a_deadline_that_does_not_move_is_found_in_debug_builds() {
    use stuck::*;
    let clock = TestClock::default();
    let mut d = Driver::<Q>::new(clock.clone(), frunk_core::hlist![()], ()).unwrap();
    clock.set(Duration::from_secs(2));
    let _ = poll(&mut d);
}
/// Release builds expire it once a poll and go on.
#[cfg(not(debug_assertions))]
#[test]
fn a_deadline_that_does_not_move_expires_once_a_poll() {
    use stuck::*;
    let clock = TestClock::default();
    let mut d = Driver::<Q>::new(clock.clone(), frunk_core::hlist![()], ()).unwrap();
    clock.set(Duration::from_secs(2));
    let _ = poll(&mut d);
    assert_eq!(EXPIRED.with(Cell::get), 1);
    let _ = poll(&mut d);
    assert_eq!(EXPIRED.with(Cell::get), 2);
}

// ---------------------------------------------------------------- why an add fails

/// An add fails for the reason it fails for.
#[test]
fn an_add_fails_for_its_reason() {
    use moves::*;
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    let a1 = d.add::<A, _, _, _>(End, 1, ()).unwrap();
    assert_eq!(
        d.add::<A, _, _, _>(End, 1, ()).err(),
        Some(AddError::Occupied {
            source: OccupiedError
        })
    );
    assert!(d.remove(a1, ()));
    assert_eq!(
        d.add::<B, _, _, _>(a1, 1, ()).err(),
        Some(AddError::NoParent {
            source: NoParentError
        })
    );
    d.end(());
    assert_eq!(d.add::<A, _, _, _>(End, 2, ()).err(), Some(AddError::Ended));
}

// ---------------------------------------------------------------- why a node ends

mod causes {
    use super::*;
    use nitori_driver::hooks::{EndCause, EndReasonOf};

    pub struct Root;
    pub struct Layer;
    impl NodeKind for Root {
        type Children = HList![Child<Layer, AtMostOne>];
        type Linked = HNil;
        type EndReason = &'static str;
    }
    impl NodeKind for Layer {
        type Children = HNil;
        type Linked = HNil;
        type EndReason = u8;
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Layer> for Base {
        type Calls = HNil;
    }
    impl Share<Root> for Base {
        type Data = HList![Told<Root>];
    }
    impl Share<Layer> for Base {
        type Data = HList![Told<Layer>];
    }
    thread_local! {
        pub static TOLD: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }
    /// Says why its node ended.
    pub struct Told<K>(core::marker::PhantomData<fn() -> K>);
    impl<L: PartList<L> + 'static, K: NodeKind<EndReason: core::fmt::Debug>> Hooks<Base, L, K>
        for Told<K>
    {
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, K>>>(
            _: BuildView<'_, Self, Base, L, K, KP>,
            _: &ArgsOf<L>,
            _: &mut nitori_driver::hooks::StartOf<K>,
        ) -> Self
        where
            K: NodeKind,
        {
            Told(core::marker::PhantomData)
        }
        fn end<KP: HookKeys<Self, Base, L, K>>(
            _: HookView<'_, Self, Base, L, K, KP>,
            cause: EndCause<'_, EndReasonOf<K>>,
        ) {
            TOLD.with(|t| t.borrow_mut().push(format!("{cause:?}")));
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
    pub fn told() -> Vec<String> {
        TOLD.with(|t| core::mem::take(&mut *t.borrow_mut()))
    }
}

/// A node ends for the reason it is given, because the node above it
/// ended, or because the driver was dropped; a linked node that its last
/// linking node ended is told so (see the requests' tests).
#[test]
fn a_node_is_told_why_it_ends() {
    use causes::*;
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    let layer = d.add::<Layer, _, _, _>(End, (), ()).unwrap();
    assert!(d.remove(layer, 7));
    assert_eq!(told(), ["Reason(7)"]);
    d.add::<Layer, _, _, _>(End, (), ()).unwrap();
    d.end("closed");
    assert_eq!(told(), ["Above", "Reason(\"closed\")"]);
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    d.add::<Layer, _, _, _>(End, (), ()).unwrap();
    drop(d);
    assert_eq!(told(), ["Dropped", "Dropped"]);
}

//! The cases the fourth review of the driver found: a node ended while it
//! is added, a node others link to asking to end itself, a node replaced
//! in the poll that runs its actions, and replacing a node that links.
#![feature(impl_trait_in_assoc_type)]
mod common;

use core::{pin::Pin, task::Poll};
use std::{
    cell::{Cell, RefCell},
    task::{Context, Waker},
};

use common::clock::TestClock;
use frunk_core::{HList, hlist::HNil, hlist_pat};
use nitori_driver::{
    call::ShareCalls,
    composition,
    driver::Driver,
    event::{Hear, NodeEnding, NodeStarted, Triggered},
    hooks::{ArgsOf, BuildKeyed, BuildView, EndCause, HookKeys, HookView, Hooks, Part, PartList},
    lend::KeyedList,
    request::{Edit, Editor, LinkedToError, NodeRequest, Request},
    structure::{
        Address, ByOwnKey, Child, End, KeyAddress, LinkedStart, NodeKind, NodePath, PathFrom,
        Share, SlotVec, Step, Store,
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

// ---------------------------------------------------------------- a node ended while it is added

mod added {
    use super::*;

    thread_local! { pub static MODE: Cell<Mode> = const { Cell::new(Mode::Keep) }; }
    #[derive(Clone, Copy)]
    pub enum Mode {
        /// The root's handler does nothing.
        Keep,
        /// It ends the leaf.
        End,
        /// It ends the leaf and adds a node of another place.
        EndAddOther,
        /// It ends the leaf and adds another leaf.
        EndAddLeaf,
    }

    pub struct Root;
    pub struct Leaf;
    pub struct Other;
    impl NodeKind for Root {
        type Children = HList![Child<Leaf, SlotVec<u32>>, Child<Other, SlotVec<u32>>];
        type Linked = HNil;
        type Request = Swap;
    }
    impl NodeKind for Leaf {
        type Children = HNil;
        type Linked = HNil;
    }
    impl NodeKind for Other {
        type Children = HNil;
        type Linked = HNil;
    }
    pub struct Swap;
    impl Request<Root> for Swap {
        fn apply<E: Editor<Root>>(self, mut edit: Edit<'_, E, Root>) {
            edit.end::<Leaf, _>(&1, ()).unwrap();
            match MODE.with(Cell::get) {
                Mode::Keep | Mode::End => {}
                Mode::EndAddOther => edit.add::<Other, _>(1, ()).unwrap(),
                Mode::EndAddLeaf => edit.add::<Leaf, _>(2, ()).unwrap(),
            }
        }
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Leaf> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Other> for Base {
        type Calls = HNil;
    }
    impl Share<Root> for Base {
        type Data = HList![Watcher];
    }
    impl Share<Leaf> for Base {
        type Data = HNil;
    }
    impl Share<Other> for Base {
        type Data = HNil;
    }
    /// Hears the first leaf start and asks the root to swap it.
    pub struct Watcher(bool);
    impl<L: PartList<L> + 'static> Hooks<Base, L, Root> for Watcher {
        type Subscribes = HList![Hear<NodeStarted<Leaf>>];
        type Returns = NodeRequest<Root, Root>;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Root>>>(
            _: BuildView<'_, Self, Base, L, Root, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Watcher(false)
        }
        fn on_events<KP: HookKeys<Self, Base, L, Root>>(
            _: Triggered<'_>,
            v: HookView<'_, Self, Base, L, Root, KP>,
        ) -> Option<Self::Returns> {
            let hlist_pat![this] = v;
            if this.0 || matches!(MODE.with(Cell::get), Mode::Keep) {
                return None;
            }
            this.0 = true;
            Some(NodeRequest::new(KeyAddress::here(), Swap))
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
}

/// The leaf added, with the root's handler in `mode`: the address of the
/// path `add` returned, whether the path reaches a node, and the driver.
fn added(mode: added::Mode) -> (Address<added::Root>, bool, Driver<added::Q>) {
    use added::*;
    MODE.with(|m| m.set(mode));
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    let path = d.add::<Leaf, _, _, _>(End, 1, ()).unwrap();
    let reached = path.node(d.root().unwrap()).is_some();
    (path.address(), reached, d)
}

/// A node a handler ends while it is added was added: `add` returns the
/// path it had, not an error.
#[test]
fn a_node_ended_while_it_is_added_was_added() {
    use added::*;
    let (kept, reached, _) = added(Mode::Keep);
    assert!(reached);
    let (path, reached, d) = added(Mode::End);
    assert_eq!(path, kept);
    assert!(!reached);
    assert!(!d.ended());
}
/// Its number given to a node of another place: the path is the leaf's,
/// and reaches no node.
#[test]
fn a_node_ended_while_it_is_added_gives_its_number_to_another_place() {
    use added::*;
    let (kept, ..) = added(Mode::Keep);
    let (path, reached, d) = added(Mode::EndAddOther);
    assert_eq!(path, kept);
    assert!(!reached);
    assert!(d.root().unwrap().children.tail.head.find(&1).is_some());
}
/// Its number given to another leaf: the path is the one the leaf had,
/// which the store gave the new leaf too.
#[test]
fn a_node_ended_while_it_is_added_gives_its_number_to_another_leaf() {
    use added::*;
    let (kept, ..) = added(Mode::Keep);
    let (path, _, d) = added(Mode::EndAddLeaf);
    assert_eq!(path, kept);
    let root = d.root().unwrap();
    assert!(root.children.head.find(&1).is_none());
    assert!(root.children.head.find(&2).is_some());
}

// ---------------------------------------------------------------- a linked node ending itself

mod linked {
    use super::*;

    thread_local! {
        pub static ASK: RefCell<Option<NodeRequest<Root, Entry>>> = const { RefCell::new(None) };
        pub static ENDED: RefCell<Option<Result<(), LinkedToError>>> = const { RefCell::new(None) };
    }
    pub struct Root;
    pub struct Entry;
    pub struct Route;
    impl NodeKind for Root {
        type Children = HList![Child<Entry, SlotVec<u32>>, Child<Route, SlotVec<u32>>];
        type Linked = HNil;
    }
    impl NodeKind for Entry {
        type Children = HNil;
        type Linked = HNil;
        type Request = EndMe;
    }
    impl NodeKind for Route {
        type Children = HNil;
        type Linked = HList![ByOwnKey<Entry>];
    }
    impl LinkedStart<Entry> for Route {
        fn linked_start(_: &mut ()) {}
    }
    /// The entry asks to end itself.
    pub struct EndMe;
    impl Request<Entry> for EndMe {
        fn apply<E: Editor<Entry>>(self, edit: Edit<'_, E, Entry>) {
            let ended = edit.end_self(());
            ENDED.with(|e| *e.borrow_mut() = Some(ended));
        }
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Entry> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Route> for Base {
        type Calls = HNil;
    }
    impl Share<Root> for Base {
        type Data = HList![Asker];
    }
    impl Share<Entry> for Base {
        type Data = HNil;
    }
    impl Share<Route> for Base {
        type Data = HNil;
    }
    /// Hands over what the test queues, before the root's calls.
    pub struct Asker;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Root> for Asker {
        const ACTS_BEFORE_CALLS: bool = true;
        type Returns = NodeRequest<Root, Entry>;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Root>>>(
            _: BuildView<'_, Self, Base, L, Root, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Asker
        }
        fn before_calls<KP: HookKeys<Self, Base, L, Root>>(
            _: HookView<'_, Self, Base, L, Root, KP>,
            _: &Waker,
        ) -> Option<Self::Returns> {
            ASK.with(|a| a.borrow_mut().take())
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
}

/// A node others link to that asks to end itself is told it was refused,
/// and ends after the last of them.
#[test]
fn a_linked_node_ending_itself_is_refused() {
    use linked::*;
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    let route = d.add::<Route, _, _, _>(End, 1, ()).unwrap();
    ASK.with(|a| {
        *a.borrow_mut() = Some(NodeRequest::new(KeyAddress::child::<Entry, _>(1), EndMe));
    });
    let _ = poll(&mut d);
    assert_eq!(ENDED.with(|e| *e.borrow()), Some(Err(LinkedToError)));
    assert!(d.root().unwrap().children.head.find(&1).is_some());
    d.remove(route, ()).unwrap();
    assert!(d.root().unwrap().children.head.find(&1).is_none());
    // one no node links to ends
    d.add::<Entry, _, _, _>(End, 2, ()).unwrap();
    ASK.with(|a| {
        *a.borrow_mut() = Some(NodeRequest::new(KeyAddress::child::<Entry, _>(2), EndMe));
    });
    let _ = poll(&mut d);
    assert_eq!(ENDED.with(|e| *e.borrow()), Some(Ok(())));
    assert!(d.root().unwrap().children.head.find(&2).is_none());
}

// ---------------------------------------------------------------- a node replaced in the poll that runs its actions

mod swapped {
    use super::*;

    thread_local! {
        pub static SWAP: Cell<bool> = const { Cell::new(false) };
        pub static RUNS: Cell<u32> = const { Cell::new(0) };
    }
    pub struct Root;
    pub struct Leaf;
    impl NodeKind for Root {
        type Children = HList![Child<Leaf, SlotVec<u32>>];
        type Linked = HNil;
        type Request = ReplaceLeaf;
    }
    impl NodeKind for Leaf {
        type Children = HNil;
        type Linked = HNil;
    }
    /// The root replaces its leaf.
    pub struct ReplaceLeaf;
    impl Request<Root> for ReplaceLeaf {
        fn apply<E: Editor<Root>>(self, mut edit: Edit<'_, E, Root>) {
            edit.replace::<Leaf, _>(1, (), ()).unwrap();
        }
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Leaf> for Base {
        type Calls = HNil;
    }
    impl Share<Root> for Base {
        type Data = HList![Swapper];
    }
    impl Share<Leaf> for Base {
        type Data = HList![Counter];
    }
    /// Asks, before the root's calls, for the leaf to be replaced when
    /// the test says so; it acts in every poll.
    pub struct Swapper;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Root> for Swapper {
        const ACTS_BEFORE_CALLS: bool = true;
        type Returns = NodeRequest<Root, Root>;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Root>>>(
            _: BuildView<'_, Self, Base, L, Root, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Swapper
        }
        fn before_calls<KP: HookKeys<Self, Base, L, Root>>(
            _: HookView<'_, Self, Base, L, Root, KP>,
            waker: &Waker,
        ) -> Option<Self::Returns> {
            waker.wake_by_ref();
            SWAP.with(|s| s.replace(false))
                .then(|| NodeRequest::new(KeyAddress::here(), ReplaceLeaf))
        }
    }
    /// Counts the runs of the leaf's action before its calls; it acts in
    /// every poll.
    pub struct Counter;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Leaf> for Counter {
        const ACTS_BEFORE_CALLS: bool = true;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Leaf>>>(
            _: BuildView<'_, Self, Base, L, Leaf, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Counter
        }
        fn before_calls<KP: HookKeys<Self, Base, L, Leaf>>(
            _: HookView<'_, Self, Base, L, Leaf, KP>,
            waker: &Waker,
        ) -> Option<Self::Returns> {
            waker.wake_by_ref();
            RUNS.with(|r| r.set(r.get() + 1));
            None
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
}

/// A node replaced by an action that runs before its own in the same
/// poll gives its number to the new node, which is not run by the old
/// one's marks: its action runs first in the next poll.
#[test]
fn a_node_replaced_in_a_poll_is_not_run_by_the_old_ones_marks() {
    use swapped::*;
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    d.add::<Leaf, _, _, _>(End, 1, ()).unwrap();
    let _ = poll(&mut d);
    assert_eq!(RUNS.with(Cell::get), 1);
    // the root acts first, the nodes above first, and replaces the leaf
    // marked in this poll
    RUNS.with(|r| r.set(0));
    SWAP.with(|s| s.set(true));
    let _ = poll(&mut d);
    assert_eq!(
        RUNS.with(Cell::get),
        0,
        "the new leaf is not run in this poll"
    );
    let _ = poll(&mut d);
    assert_eq!(RUNS.with(Cell::get), 1, "it is run in the next one");
}

// ---------------------------------------------------------------- replacing a node that links

mod relinked {
    use super::*;

    thread_local! {
        pub static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        pub static ASK: RefCell<Option<ReplaceRoute>> = const { RefCell::new(None) };
    }
    pub fn put(s: impl Into<String>) {
        LOG.with(|l| l.borrow_mut().push(s.into()));
    }
    pub fn log() -> Vec<String> {
        LOG.with(|l| core::mem::take(&mut *l.borrow_mut()))
    }
    pub struct Root;
    pub struct Peer;
    /// Two kinds of node that link to a peer by their own key.
    pub struct Route;
    pub struct Path;
    impl NodeKind for Root {
        type Children = HList![
            Child<Peer, SlotVec<u32>>,
            Child<Route, SlotVec<u32>>,
            Child<Path, SlotVec<u32>>,
        ];
        type Linked = HNil;
        type Request = ReplaceRoute;
    }
    impl NodeKind for Peer {
        type Children = HNil;
        type Linked = HNil;
    }
    impl NodeKind for Route {
        type Children = HNil;
        type Linked = HList![ByOwnKey<Peer>];
    }
    impl LinkedStart<Peer> for Route {
        fn linked_start(_: &mut ()) {
            put("linked start");
        }
    }
    impl NodeKind for Path {
        type Children = HNil;
        type Linked = HList![ByOwnKey<Peer>];
    }
    impl LinkedStart<Peer> for Path {
        fn linked_start(_: &mut ()) {
            put("linked start");
        }
    }
    /// The root replaces the route under the key.
    pub struct ReplaceRoute(pub u32);
    impl Request<Root> for ReplaceRoute {
        fn apply<E: Editor<Root>>(self, mut edit: Edit<'_, E, Root>) {
            edit.replace::<Route, _>(self.0, (), ()).unwrap();
        }
    }
    impl ShareCalls<Root> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Peer> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Route> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Path> for Base {
        type Calls = HNil;
    }
    impl Share<Root> for Base {
        type Data = HList![Watcher];
    }
    impl Share<Peer> for Base {
        type Data = HList![PeerLog];
    }
    impl Share<Route> for Base {
        type Data = HNil;
    }
    impl Share<Path> for Base {
        type Data = HNil;
    }
    /// Hears the peers start and end, and asks what the test queues.
    pub struct Watcher;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Root> for Watcher {
        const ACTS_BEFORE_CALLS: bool = true;
        type Subscribes = HList![Hear<NodeStarted<Peer>>, Hear<NodeEnding<Peer>>];
        type Returns = NodeRequest<Root, Root>;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Root>>>(
            _: BuildView<'_, Self, Base, L, Root, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Watcher
        }
        fn on_events<KP: HookKeys<Self, Base, L, Root>>(
            events: Triggered<'_>,
            _: HookView<'_, Self, Base, L, Root, KP>,
        ) -> Option<Self::Returns> {
            if events.contains::<NodeStarted<Peer>>() {
                put("peer started");
            }
            if events.contains::<NodeEnding<Peer>>() {
                put("peer ending");
            }
            None
        }
        fn before_calls<KP: HookKeys<Self, Base, L, Root>>(
            _: HookView<'_, Self, Base, L, Root, KP>,
            waker: &Waker,
        ) -> Option<Self::Returns> {
            waker.wake_by_ref();
            ASK.with(|a| a.borrow_mut().take())
                .map(|r| NodeRequest::new(KeyAddress::here(), r))
        }
    }
    /// Logs a peer's build and end.
    pub struct PeerLog;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Peer> for PeerLog {
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Peer>>>(
            _: BuildView<'_, Self, Base, L, Peer, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            put("peer built");
            PeerLog
        }
        fn end<KP: HookKeys<Self, Base, L, Peer>>(
            _: HookView<'_, Self, Base, L, Peer, KP>,
            cause: EndCause<'_, ()>,
        ) {
            put(format!("peer ended {cause:?}"));
        }
    }
    pub struct Q;
    composition!(impl driver for Q: Root, HList![Base], TestClock);
    /// The path of a route.
    pub type RouteAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
}

/// Replacing the last node that links to a peer keeps the peer: it does
/// not end or start again, and the new node links to it.
#[test]
fn replacing_the_last_node_linking_to_a_peer_keeps_it() {
    use relinked::*;
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    d.add::<Route, _, _, _>(End, 1, ()).unwrap();
    let _ = poll(&mut d);
    let peer = d.root().unwrap().children.head.find(&1).unwrap();
    assert_eq!(log(), ["linked start", "peer built", "peer started"]);
    ASK.with(|a| *a.borrow_mut() = Some(ReplaceRoute(1)));
    let _ = poll(&mut d);
    assert!(log().is_empty(), "the peer did not end or start");
    assert_eq!(d.root().unwrap().children.head.find(&1), Some(peer));
    // it is linked to by the new route: removing that one ends it
    let route = d.root().unwrap().children.tail.head.find(&1).unwrap();
    let route: RouteAt = Step::new(route, End);
    d.remove(route, ()).unwrap();
    assert_eq!(log(), ["peer ended Unlinked", "peer ending"]);
}

/// Replacing a node that links to a peer another node links to keeps the
/// peer, which ends after the last of them.
#[test]
fn replacing_a_node_linking_to_a_shared_peer_keeps_it() {
    use relinked::*;
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], ()).unwrap();
    d.add::<Route, _, _, _>(End, 1, ()).unwrap();
    let path = d.add::<Path, _, _, _>(End, 1, ()).unwrap();
    let _ = poll(&mut d);
    let peer = d.root().unwrap().children.head.find(&1).unwrap();
    assert_eq!(log(), ["linked start", "peer built", "peer started"]);
    ASK.with(|a| *a.borrow_mut() = Some(ReplaceRoute(1)));
    let _ = poll(&mut d);
    assert!(log().is_empty(), "the peer did not end or start");
    assert_eq!(d.root().unwrap().children.head.find(&1), Some(peer));
    d.remove(path, ()).unwrap();
    assert!(log().is_empty(), "the new route still links to it");
    assert_eq!(d.root().unwrap().children.head.find(&1), Some(peer));
}

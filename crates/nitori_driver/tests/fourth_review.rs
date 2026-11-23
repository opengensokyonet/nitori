//! The cases the fourth review of the driver found: a node ended while it
//! is added, a node others link to asking to end itself, why a removal
//! fails, a move of a grandchild that names the wrong child, and a node
//! replaced in the poll that runs its actions.
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
    event::{Hear, NodeStarted, Triggered},
    hooks::{ArgsOf, BuildKeyed, BuildView, HookKeys, HookView, Hooks, Part, PartList},
    lend::KeyedList,
    request::{Edit, Editor, LinkedToError, NodeRequest, Request},
    structure::{
        Address, ByOwnKey, Child, End, KeyAddress, LinkedStart, NodeKind, NodePath, PathFrom,
        Share, SlotVec, Store,
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
    assert!(d.remove(route, ()));
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

//! The cases the fourth review of the driver found: a node ended while it
//! is added, a node others link to asking to end itself, why a removal
//! fails, a move of a grandchild that names the wrong child, and a node
//! replaced in the poll that runs its actions.
#![feature(impl_trait_in_assoc_type)]
mod common;

use std::cell::Cell;

use common::clock::TestClock;
use frunk_core::{HList, hlist::HNil, hlist_pat};
use nitori_driver::{
    call::ShareCalls,
    composition,
    driver::Driver,
    event::{Hear, NodeStarted, Triggered},
    hooks::{ArgsOf, BuildKeyed, BuildView, HookKeys, HookView, Hooks, Part, PartList},
    lend::KeyedList,
    request::{Edit, Editor, NodeRequest, Request},
    structure::{
        Address, Child, End, KeyAddress, NodeKind, NodePath, PathFrom, Share, SlotVec, Store,
    },
};

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

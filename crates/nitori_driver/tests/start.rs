//! What a node starts from: the start the one adding it gives, through
//! `Driver::new`, `Driver::add` and a request's edit handle, to every
//! component's `build` in turn; the start of the node a node links to; and
//! the key a node is kept under, read through the need `OwnKey`.
#![feature(impl_trait_in_assoc_type)]
mod common;

use core::{pin::Pin, task::Poll};
use std::{
    cell::RefCell,
    task::{Context, Waker},
};

use common::clock::TestClock;
use frunk_core::{HList, hlist::HNil, hlist_pat};
use nitori_driver::{
    call::ShareCalls,
    composition,
    driver::Driver,
    hooks::{ArgsOf, BuildKeyed, BuildView, HookKeys, HookView, Hooks, Part, PartList},
    lend::KeyedList,
    need::{At, Here, OwnKey, Up},
    request::{Edit, Editor, NodeRequest, Request},
    structure::{ByOwnKey, Child, End, KeyAddress, LinkedStart, NodeKind, Share, SlotVec},
};

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static ASKED: RefCell<Vec<NodeRequest<Root, Root>>> = const { RefCell::new(Vec::new()) };
    static ASKER: RefCell<Option<Waker>> = const { RefCell::new(None) };
}
fn put(what: impl Into<String>) {
    LOG.with(|l| l.borrow_mut().push(what.into()));
}
fn log() -> Vec<String> {
    LOG.with(|l| core::mem::take(&mut *l.borrow_mut()))
}

/// A value that moves once.
#[derive(Debug, PartialEq, Eq)]
pub struct Token(u32);

pub struct Root;
pub struct Item;
pub struct Peer;
impl NodeKind for Root {
    type Children = HList![Child<Item, SlotVec<u32>>, Child<Peer, SlotVec<u32>>];
    type Linked = HNil;
    type Start = u8;
    type Request = RootRequest;
}
/// What an item starts from: a name every component reads, and a token
/// the first takes.
pub struct ItemStart {
    pub name: &'static str,
    pub token: Option<Token>,
}
impl NodeKind for Item {
    type Children = HNil;
    type Linked = HList![ByOwnKey<Peer>];
    type Start = ItemStart;
}
impl LinkedStart<Peer> for Item {
    fn linked_start(start: &mut ItemStart) -> String {
        put("linked start");
        format!("peer of {}", start.name)
    }
}
impl NodeKind for Peer {
    type Children = HNil;
    type Linked = HNil;
    type Start = String;
}

/// What the root is asked: add or replace an item.
pub enum RootRequest {
    Add(u32, ItemStart),
    Replace(u32, ItemStart),
}
impl Request<Root> for RootRequest {
    fn apply<E: Editor>(self, mut edit: Edit<'_, E, Root>) {
        let done = match self {
            Self::Add(k, s) => edit.add::<Item, _>(k, s),
            Self::Replace(k, s) => edit.replace::<Item, _>(k, s, ()),
        };
        put(format!("done {done}"));
    }
}

pub struct Base;
impl<L> Part<L> for Base {
    type Args = ();
}
impl ShareCalls<Root> for Base {
    type Calls = HNil;
}
impl ShareCalls<Item> for Base {
    type Calls = HNil;
}
impl ShareCalls<Peer> for Base {
    type Calls = HNil;
}
impl Share<Root> for Base {
    type Data = HList![Asker];
}
impl Share<Item> for Base {
    type Data = HList![First, Second];
}
impl Share<Peer> for Base {
    type Data = HList![PeerName];
}

/// On the root: its start, and the requests the test asks.
pub struct Asker(u8);
impl<L: PartList<L> + 'static> Hooks<Base, L, Root> for Asker {
    const ACTS_BEFORE_CALLS: bool = true;
    type Returns = NodeRequest<Root, Root>;
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Root>>>(
        _: BuildView<'_, Self, Base, L, Root, KP>,
        _: &ArgsOf<L>,
        start: &mut u8,
    ) -> Self {
        Asker(*start)
    }
    fn before_calls<KP: HookKeys<Self, Base, L, Root>>(
        _: HookView<'_, Self, Base, L, Root, KP>,
        waker: &Waker,
    ) -> Option<NodeRequest<Root, Root>> {
        ASKER.with(|a| *a.borrow_mut() = Some(waker.clone()));
        ASKED.with(|a| a.borrow_mut().pop())
    }
}

/// The first component of an item: takes the token, reads its own key
/// when built and when it ends.
pub struct First(Option<Token>);
impl<L: PartList<L> + 'static> Hooks<Base, L, Item> for First {
    type Needs = HList![At<Up<0>, Here, OwnKey<u32>>];
    type BuildNeeds = HList![At<Up<0>, Here, OwnKey<u32>>];
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Item>>>(
        parents: BuildView<'_, Self, Base, L, Item, KP>,
        _: &ArgsOf<L>,
        start: &mut ItemStart,
    ) -> Self {
        let hlist_pat![key] = parents;
        put(format!("first {} {key}", start.name));
        First(start.token.take())
    }
    fn end<KP: HookKeys<Self, Base, L, Item>>(view: HookView<'_, Self, Base, L, Item, KP>, _: &()) {
        let hlist_pat![this, key] = view;
        put(format!("end {key} {:?}", this.0));
    }
}
/// The second: what the first left of the start.
pub struct Second;
impl<L: PartList<L> + 'static> Hooks<Base, L, Item> for Second {
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Item>>>(
        _: BuildView<'_, Self, Base, L, Item, KP>,
        _: &ArgsOf<L>,
        start: &mut ItemStart,
    ) -> Self {
        put(format!("second {} {:?}", start.name, start.token));
        Second
    }
}
/// A peer's start, and its key.
pub struct PeerName;
impl<L: PartList<L> + 'static> Hooks<Base, L, Peer> for PeerName {
    type BuildNeeds = HList![At<Up<0>, Here, OwnKey<u32>>];
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Peer>>>(
        parents: BuildView<'_, Self, Base, L, Peer, KP>,
        _: &ArgsOf<L>,
        start: &mut String,
    ) -> Self {
        let hlist_pat![key] = parents;
        put(format!("peer {key} {start}"));
        PeerName
    }
}

pub struct Q;
composition!(impl driver for Q: Root, HList![Base], TestClock);

fn poll(d: &mut Driver<Q>) -> Poll<()> {
    Pin::new(d).poll(&mut Context::from_waker(Waker::noop()))
}
fn ask(d: &mut Driver<Q>, r: RootRequest) {
    ASKED.with(|a| a.borrow_mut().push(NodeRequest::new(KeyAddress::here(), r)));
    if let Some(w) = ASKER.with(|a| a.borrow_mut().take()) {
        w.wake();
    }
    let _ = poll(d);
}
fn start(name: &'static str, token: u32) -> ItemStart {
    ItemStart {
        name,
        token: Some(Token(token)),
    }
}

#[test]
fn the_root_from_its_start() {
    let d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], 7).unwrap();
    assert_eq!(d.root().unwrap().data.head.head.0, 7);
}

#[test]
fn a_start_reaches_every_component_and_moves_once() {
    let _ = log();
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], 0).unwrap();
    let _ = poll(&mut d);
    d.add::<Item, _, _>(End, 3, start("three", 30)).unwrap();
    assert_eq!(
        log(),
        [
            "linked start",
            "peer 3 peer of three",
            "first three 3",
            "second three None",
        ]
    );
    // through a request
    ask(&mut d, RootRequest::Add(4, start("four", 40)));
    assert_eq!(
        log(),
        [
            "linked start",
            "peer 4 peer of four",
            "first four 4",
            "second four None",
            "done true",
        ]
    );
    // the old item ends, reading its key, and the peer it was the last
    // to link to ends after it; the new one starts from the start given
    // and builds its peer again
    ask(&mut d, RootRequest::Replace(4, start("again", 41)));
    assert_eq!(
        log(),
        [
            "end 4 Some(Token(40))",
            "linked start",
            "peer 4 peer of again",
            "first again 4",
            "second again None",
            "done true",
        ]
    );
}

#[test]
fn a_linked_node_there_already_keeps_its_own_start() {
    let _ = log();
    let mut d = Driver::<Q>::new(TestClock::default(), frunk_core::hlist![()], 0).unwrap();
    d.add::<Peer, _, _>(End, 5, "given".into()).unwrap();
    d.add::<Item, _, _>(End, 5, start("five", 50)).unwrap();
    assert_eq!(log(), ["peer 5 given", "first five 5", "second five None"]);
}

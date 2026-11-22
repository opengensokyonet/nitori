//! Component hooks: a component generic over the list of parts, a generic
//! type of another crate given hooks by a part, a host's type in `Held`,
//! the arguments of every part, the hooks called share by share.
use std::{cell::RefCell, task::Waker};

use frunk_core::{
    HList,
    hlist::{HCons, HNil},
    hlist_pat,
};
use nitori_driver::{
    hooks::{
        AbortCause, ArgsOf, DataHooks, Held, Hooks, Part, PartList, ShareHooks, build_node,
        end_node,
    },
    structure::{AtMostOne, Child, KindFor, NodeKind, Share},
};

/// A connection with one path (at most one).
pub struct Connection;
pub struct Path;
impl NodeKind for Connection {
    type Children = HList![Child<Path, AtMostOne>];
    type Linked = HNil;
}
impl NodeKind for Path {
    type Children = HNil;
    type Linked = HNil;
}

thread_local! {
    static LOG: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}
fn put(what: &'static str) {
    LOG.with(|l| l.borrow_mut().push(what));
}
fn log() -> Vec<&'static str> {
    LOG.with(|l| core::mem::take(&mut *l.borrow_mut()))
}

// ---------------------------------------------------------------- parts and their arguments

pub struct Base;
pub struct BaseArgs {
    pub window: u64,
}
impl<L> Part<L> for Base {
    type Args = BaseArgs;
}
pub struct Reset;
pub struct ResetArgs {
    pub enabled: bool,
}
impl<L> Part<L> for Reset {
    type Args = ResetArgs;
}
/// A part whose arguments depend on the whole list: how many parts there
/// are.
pub struct Counted;
pub struct CountedArgs<L>(pub core::marker::PhantomData<fn() -> L>);
impl<L> Part<L> for Counted {
    type Args = CountedArgs<L>;
}

/// What the streams read of the arguments, part by part.
pub trait StreamArgs {
    fn window(&self) -> Option<u64> {
        None
    }
    fn reliable(&self) -> bool {
        false
    }
}
impl StreamArgs for BaseArgs {
    fn window(&self) -> Option<u64> {
        Some(self.window)
    }
}
impl StreamArgs for ResetArgs {
    fn reliable(&self) -> bool {
        self.enabled
    }
}
impl<L> StreamArgs for CountedArgs<L> {}
/// The same of the whole list, from its parts' answers.
pub trait StreamArgsList {
    fn window(&self) -> Option<u64>;
    fn reliable(&self) -> bool;
}
impl StreamArgsList for HNil {
    fn window(&self) -> Option<u64> {
        None
    }
    fn reliable(&self) -> bool {
        false
    }
}
impl<A: StreamArgs, T: StreamArgsList> StreamArgsList for HCons<A, T> {
    fn window(&self) -> Option<u64> {
        self.head.window().or(self.tail.window())
    }
    fn reliable(&self) -> bool {
        self.head.reliable() || self.tail.reliable()
    }
}

// ---------------------------------------------------------------- components

/// Generic over the list of parts: it reads the list's arguments.
#[derive(Debug, PartialEq, Eq)]
pub struct Streams {
    pub window: u64,
    pub reliable: bool,
}
impl<O, L: PartList<L, Args: StreamArgsList>, K> Hooks<O, L, K> for Streams {
    const ACTS_BEFORE_CALLS: bool = true;
    fn build(args: &ArgsOf<L>) -> Self {
        Streams {
            window: args.window().expect("the base part gives the window"),
            reliable: args.reliable(),
        }
    }
    fn before_calls(&mut self, _: &Waker) {
        put("streams before calls");
    }
    fn aborted(&mut self, cause: AbortCause) {
        put(match cause {
            AbortCause::Dropped => "streams dropped",
            AbortCause::Panicked => "streams panicked",
        });
    }
    fn end(self) {
        put("streams end");
    }
}

/// A generic type of another crate, given hooks by the base part: the
/// part comes before the list, so the impl is allowed.
impl<X: Default, L: PartList<L>, K> Hooks<Base, L, K> for Vec<X> {
    fn build(_: &ArgsOf<L>) -> Self {
        vec![X::default()]
    }
    fn end(self) {
        put("vec end");
    }
}

/// A type the host chooses, behind `Held`.
pub trait Estimator {
    fn new() -> Self;
    fn name(&self) -> &'static str;
}
pub struct Smoothed;
impl Estimator for Smoothed {
    fn new() -> Self {
        Smoothed
    }
    fn name(&self) -> &'static str {
        "smoothed"
    }
}
impl<X: Estimator, L: PartList<L>, K> Hooks<Base, L, K> for Held<X> {
    fn build(_: &ArgsOf<L>) -> Self {
        Held(X::new())
    }
    fn end(self) {
        put(self.0.name());
    }
}

/// The reliable reset part's flag; it acts on nothing.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ResetFlag(pub bool);
impl<L: PartList<L>, K> Hooks<Reset, L, K> for ResetFlag {
    fn build(_: &ArgsOf<L>) -> Self {
        ResetFlag(true)
    }
}

impl Share<Connection> for Base {
    type Data = HList![Streams, Vec<u8>, Held<Smoothed>];
}
impl Share<Path> for Base {
    type Data = HList![Vec<u16>];
}
impl Share<Connection> for Reset {
    type Data = HList![ResetFlag];
}
impl Share<Path> for Reset {
    type Data = HNil;
}
impl Share<Connection> for Counted {
    type Data = HList![Streams];
}
impl Share<Path> for Counted {
    type Data = HNil;
}

type Parts = HList![Base, Reset];

fn args() -> ArgsOf<Parts> {
    frunk_core::hlist![BaseArgs { window: 1000 }, ResetArgs { enabled: true }]
}

#[test]
fn build_a_node_from_every_part_s_arguments() {
    let root = build_node::<Connection, Parts>(&args());
    let hlist_pat![hlist_pat![streams, v, held], hlist_pat![flag]] = &root.data;
    assert_eq!(
        streams,
        &Streams {
            window: 1000,
            reliable: true
        }
    );
    assert_eq!(v, &vec![0u8]);
    assert_eq!(held.0.name(), "smoothed");
    assert_eq!(flag, &ResetFlag(true));
    // Without the reliable reset part the same component is built
    // otherwise.
    let alone = build_node::<Connection, HList![Base]>(&frunk_core::hlist![BaseArgs { window: 5 }]);
    assert_eq!(
        alone.data.head.head,
        Streams {
            window: 5,
            reliable: false
        }
    );
}

#[test]
fn hooks_called_share_by_share() {
    let mut root = build_node::<Connection, Parts>(&args());
    const ACTS: bool = <<Connection as KindFor<Parts>>::Data as DataHooks<
        Parts,
        Parts,
        Connection,
    >>::ACTS_BEFORE_CALLS;
    const { assert!(ACTS) };
    const PATH_ACTS: bool =
        <<Path as KindFor<Parts>>::Data as DataHooks<Parts, Parts, Path>>::ACTS_BEFORE_CALLS;
    const { assert!(!PATH_ACTS) };
    let waker = Waker::noop();
    type Data = <Connection as KindFor<Parts>>::Data;
    <Data as DataHooks<Parts, Parts, Connection>>::before_calls(&mut root.data, waker);
    <Data as DataHooks<Parts, Parts, Connection>>::aborted(&mut root.data, AbortCause::Dropped);
    assert_eq!(log(), ["streams before calls", "streams dropped"]);
    end_node(root);
    assert_eq!(log(), ["streams end", "vec end", "smoothed"]);
    // A share on its own.
    let mut share =
        <HList![Streams] as ShareHooks<Counted, HList![Base, Counted], Connection>>::build(
            &frunk_core::hlist![
                BaseArgs { window: 2 },
                CountedArgs(core::marker::PhantomData)
            ],
        );
    <HList![Streams] as ShareHooks<Counted, HList![Base, Counted], Connection>>::aborted(
        &mut share,
        AbortCause::Panicked,
    );
    assert_eq!(log(), ["streams panicked"]);
}

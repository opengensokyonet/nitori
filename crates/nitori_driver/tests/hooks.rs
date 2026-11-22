//! Component hooks on an HTTP-like structure: a component generic over
//! the list of parts, a generic type of another crate given hooks by a
//! part, a host's type in `Held`, the arguments of every part, `build`
//! from the parent's view, events heard from below, beside (by anchor and
//! steps) and from above.
#![feature(impl_trait_in_assoc_type)]
mod common;

use core::{pin::Pin, task::Poll};
use std::{
    cell::RefCell,
    task::{Context, Waker},
};

use common::{clock::TestClock, http::*};
use frunk_core::{
    HList,
    hlist::{HCons, HNil},
    hlist_pat,
};
use nitori_call::{LentAccess, call};
use nitori_driver::{
    call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
    composition,
    driver::Driver,
    event::{Event, Hear, HearAt, NodeStarted, Publish, Triggered},
    hooks::{
        AbortCause, ArgsOf, BuildKeyed, BuildView, EndReasonOf, Held, HookKeys, HookView, Hooks,
        Part, PartList,
    },
    lend::KeyedList,
    need::{At, ByKey, Here, Kind, Mut, Ref, ToChild, Up},
    structure::{End, NodePath, Share},
};

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

// ---------------------------------------------------------------- events

/// Data arrived on a request stream, at the stream.
pub struct DataArrived;
impl Event for DataArrived {}

// ---------------------------------------------------------------- components

/// Generic over the part, the list and the kind: it reads the list's
/// arguments, and acts before the calls.
#[derive(Debug, PartialEq, Eq)]
pub struct Streams {
    pub window: u64,
    pub reliable: bool,
}
impl<O: 'static, L: PartList<L, Args: StreamArgsList> + 'static, K: 'static> Hooks<O, L, K>
    for Streams
{
    const ACTS_BEFORE_CALLS: bool = true;
    fn build<KP: KeyedList<L, BuildKeyed<Self, O, L, K>>>(
        _: BuildView<'_, Self, O, L, K, KP>,
        args: &ArgsOf<L>,
    ) -> Self {
        Streams {
            window: args.window().expect("the base part gives the window"),
            reliable: args.reliable(),
        }
    }
    fn before_calls<KP: HookKeys<Self, O, L, K>>(
        _: HookView<'_, Self, O, L, K, KP>,
        _: &Waker,
    ) -> Option<Self::Returns> {
        put("streams before calls");
        None
    }
    fn aborted<KP: HookKeys<Self, O, L, K>>(_: HookView<'_, Self, O, L, K, KP>, cause: AbortCause) {
        put(match cause {
            AbortCause::Dropped => "streams dropped",
            AbortCause::Panicked => "streams panicked",
        });
    }
    fn end<KP: HookKeys<Self, O, L, K>>(_: HookView<'_, Self, O, L, K, KP>, _: &EndReasonOf<K>)
    where
        K: nitori_driver::structure::NodeKind,
    {
        put("streams end");
    }
}

/// A generic type of another crate, given hooks by the base part: the
/// part comes before the list, so the impl is allowed.
impl<X: Default + 'static, L: PartList<L> + 'static, K: 'static> Hooks<Base, L, K> for Vec<X> {
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, K>>>(
        _: BuildView<'_, Self, Base, L, K, KP>,
        _: &ArgsOf<L>,
    ) -> Self {
        vec![X::default()]
    }
    fn end<KP: HookKeys<Self, Base, L, K>>(
        _: HookView<'_, Self, Base, L, K, KP>,
        _: &EndReasonOf<K>,
    ) where
        K: nitori_driver::structure::NodeKind,
    {
        put("vec end");
    }
}

/// A type the host chooses, behind `Held`.
pub trait Estimator: 'static {
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
impl<X: Estimator, L: PartList<L> + 'static, K: 'static> Hooks<Base, L, K> for Held<X> {
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, K>>>(
        _: BuildView<'_, Self, Base, L, K, KP>,
        _: &ArgsOf<L>,
    ) -> Self {
        Held(X::new())
    }
    fn end<KP: HookKeys<Self, Base, L, K>>(
        view: HookView<'_, Self, Base, L, K, KP>,
        _: &EndReasonOf<K>,
    ) where
        K: nitori_driver::structure::NodeKind,
    {
        put(view.head.0.name());
    }
}

/// The reliable reset part's flag.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ResetFlag(pub bool);
impl<L: PartList<L> + 'static, K: 'static> Hooks<Reset, L, K> for ResetFlag {
    fn build<KP: KeyedList<L, BuildKeyed<Self, Reset, L, K>>>(
        _: BuildView<'_, Self, Reset, L, K, KP>,
        _: &ArgsOf<L>,
    ) -> Self {
        ResetFlag(true)
    }
}

/// On the control stream: how many request streams started, heard from
/// beside it through its anchor and steps.
#[derive(Debug, Default)]
pub struct ControlSeen(pub u64);
impl<L: PartList<L> + 'static> Hooks<Base, L, Control> for ControlSeen {
    type Subscribes =
        HList![HearAt<NodeStarted<Stream>, Kind<HttpConnection>, ToChild<Stream, ByKey<StreamId>>>];
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Control>>>(
        _: BuildView<'_, Self, Base, L, Control, KP>,
        _: &ArgsOf<L>,
    ) -> Self {
        ControlSeen(0)
    }
    fn on_events<KP: HookKeys<Self, Base, L, Control>>(
        _: Triggered<'_>,
        view: HookView<'_, Self, Base, L, Control, KP>,
    ) -> Option<Self::Returns> {
        view.head.0 += 1;
        None
    }
}

/// On a request stream: headers, the first one the connection's window,
/// read from the parent when the stream is built.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StreamHeaders(pub Vec<u64>);
impl<L: PartList<L> + 'static> Hooks<Base, L, Stream> for StreamHeaders {
    type BuildNeeds = HList![At<Up<0>, Here, Ref<Streams>>];
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Stream>>>(
        parents: BuildView<'_, Self, Base, L, Stream, KP>,
        _: &ArgsOf<L>,
    ) -> Self {
        let hlist_pat![streams] = parents;
        StreamHeaders(vec![streams.window])
    }
}

/// On a body: how much arrived, heard from the stream above it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BodyReceived(pub u64);
impl<L: PartList<L> + 'static> Hooks<Base, L, Body> for BodyReceived {
    type Subscribes = HList![Hear<DataArrived>];
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Body>>>(
        _: BuildView<'_, Self, Base, L, Body, KP>,
        _: &ArgsOf<L>,
    ) -> Self {
        BodyReceived(0)
    }
    fn on_events<KP: HookKeys<Self, Base, L, Body>>(
        events: Triggered<'_>,
        view: HookView<'_, Self, Base, L, Body, KP>,
    ) -> Option<Self::Returns> {
        assert!(events.contains::<DataArrived>());
        view.head.0 += 1;
        None
    }
}

impl Share<HttpConnection> for Base {
    type Data = HList![Streams, Vec<u8>, Held<Smoothed>];
}
impl Share<Control> for Base {
    type Data = HList![ControlSeen];
}
impl Share<Stream> for Base {
    type Data = HList![StreamHeaders];
}
impl Share<Body> for Base {
    type Data = HList![BodyReceived];
}
impl Share<HttpConnection> for Reset {
    type Data = HList![ResetFlag];
}
impl Share<Control> for Reset {
    type Data = HNil;
}
impl Share<Stream> for Reset {
    type Data = HNil;
}
impl Share<Body> for Reset {
    type Data = HNil;
}
macro_rules! no_calls {
    ($part:ty: $($k:ty),*) => {$(
        impl ShareCalls<$k> for $part {
            type Calls = HNil;
        }
    )*};
}
no_calls!(Base: HttpConnection, Control, Body);
no_calls!(Reset: HttpConnection, Control, Stream, Body);
impl ShareCalls<Stream> for Base {
    type Calls = HList![Receive];
}

/// On a request stream: data arrives once, published at the stream.
pub struct Receive;
impl<Q: Composition> CallKind<Q> for Receive {
    type Needs = HList![
        At<Up<0>, Here, Mut<StreamHeaders>>,
        At<Up<0>, Here, Publish<DataArrived>>
    ];
    type Op = Receiving;
    fn start() -> Receiving {
        Receiving::new()
    }
}
#[call]
pub async fn receiving<W: CallFamily<C = Receive>>(host: &mut impl LentAccess<Family = W>) {
    host.with(|mut view| {
        let hlist_pat![headers, arrived] = view.needs();
        headers.0.push(1);
        arrived.publish();
    });
}

pub struct H;
composition!(impl driver for H: HttpConnection, HList![Base, Reset], TestClock);
pub struct HAlone;
composition!(impl driver for HAlone: HttpConnection, HList![Base], TestClock);

fn args() -> ArgsOf<HList![Base, Reset]> {
    frunk_core::hlist![BaseArgs { window: 1000 }, ResetArgs { enabled: true }]
}
fn poll(d: &mut Driver<H>) -> Poll<()> {
    Pin::new(d).poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn the_root_from_every_part_s_arguments() {
    let d = Driver::<H>::new(TestClock::default(), args()).unwrap();
    let hlist_pat![hlist_pat![streams, v, held], hlist_pat![flag]] = &d.root().unwrap().data;
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
    let alone = Driver::<HAlone>::new(
        TestClock::default(),
        frunk_core::hlist![BaseArgs { window: 5 }],
    )
    .unwrap();
    assert_eq!(
        alone.root().unwrap().data.head.head,
        Streams {
            window: 5,
            reliable: false
        }
    );
}

#[test]
fn hooks_of_every_share_part_by_part() {
    log();
    let mut d = Driver::<H>::new(TestClock::default(), args()).unwrap();
    let _ = poll(&mut d);
    assert_eq!(log(), ["streams before calls"]);
    drop(d);
    assert_eq!(
        log(),
        ["streams dropped", "streams end", "vec end", "smoothed"]
    );
}

#[test]
fn streams_built_from_the_parent_events_from_beside_and_above() {
    log();
    let mut d = Driver::<H>::new(TestClock::default(), args()).unwrap();
    let control = d.add::<Control, _, _>(End, ()).unwrap();
    let s1 = d.add::<Stream, _, _>(End, StreamId(1)).unwrap();
    let s2 = d.add::<Stream, _, _>(End, StreamId(2)).unwrap();
    let b1 = d.add::<Body, _, _>(s1, ()).unwrap();
    let root = d.root().unwrap();
    assert_eq!(
        s1.node(root).unwrap().data.head.head,
        StreamHeaders(vec![1000])
    );
    // the control stream beside them heard both start
    assert_eq!(control.node(root).unwrap().data.head.head.0, 2);
    let _ = poll(&mut d);
    let root = d.root().unwrap();
    assert_eq!(
        s1.node(root).unwrap().data.head.head,
        StreamHeaders(vec![1000, 1])
    );
    assert_eq!(
        s2.node(root).unwrap().data.head.head,
        StreamHeaders(vec![1000, 1])
    );
    // data at the first stream reaches its body below it, once; the
    // second stream's data does not reach it
    assert_eq!(b1.node(root).unwrap().data.head.head, BodyReceived(1));
    // a body added later hears what arrives from then on only
    let b2 = d.add::<Body, _, _>(s2, ()).unwrap();
    assert_eq!(
        b2.node(d.root().unwrap()).unwrap().data.head.head,
        BodyReceived(0)
    );
}

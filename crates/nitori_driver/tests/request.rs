//! Requests on a connection-like structure: each kind of node handles its
//! own, on its own subtree, through the edit handle; calls yield them and
//! hooks return them, and the driver handles them before it polls another
//! call.
//!
//! The connection has an established layer (at most one), entries by link
//! and logical paths by key; a logical path has link paths by link, each
//! linked to the connection's entry of its own link; a link path has a
//! recovery layer (at most one).
#![feature(impl_trait_in_assoc_type, yield_expr)]
mod common;

use core::{fmt::Debug, future::poll_fn, marker::PhantomData, pin::Pin, task::Poll};
use std::{
    cell::RefCell,
    collections::VecDeque,
    task::{Context, Waker},
    time::Duration,
};

use common::clock::TestClock;
use frunk_core::{Coprod, HList, coproduct::Coproduct, hlist::HNil};
use nitori_call::{LentAccess, call};
use nitori_driver::{
    call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
    composition,
    driver::Driver,
    event::{Hear, NodeStarted, Triggered},
    hooks::{
        ArgsOf, BuildKeyed, BuildView, EndReasonOf, HasDeadline, HookKeys, HookView, Hooks, Part,
        PartList, StartOf, Timed,
    },
    lend::KeyedList,
    need::{At, Here, Kind, MyLink, Now, Ref, ToChild, Up},
    request::{Edit, Editor, NodeRequest, Request},
    structure::{AtMostOne, ByOwnKey, Child, KeyAddress, NodeKind, Share, SlotVec, Store},
};

// ---------------------------------------------------------------- a log

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static SCRIPT: RefCell<VecDeque<Sent>> = const { RefCell::new(VecDeque::new()) };
    static SCRIPT_WAKER: RefCell<Option<Waker>> = const { RefCell::new(None) };
    static RECOVER_WAKERS: RefCell<Vec<Waker>> = const { RefCell::new(Vec::new()) };
    static OUTER: Waker = Waker::from(std::sync::Arc::new(Outer::default()));
}
/// The outer waker, counting its wakes: one for every poll, so that the
/// driver keeps its calls' wakers.
#[derive(Default)]
struct Outer(std::sync::atomic::AtomicUsize);
impl std::task::Wake for Outer {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
fn put(what: impl Into<String>) {
    LOG.with(|l| l.borrow_mut().push(what.into()));
}
fn log() -> Vec<String> {
    LOG.with(|l| core::mem::take(&mut *l.borrow_mut()))
}
/// Have the script call yield `requests`, in order.
fn script(requests: impl IntoIterator<Item = Sent>) {
    SCRIPT.with(|s| s.borrow_mut().extend(requests));
    if let Some(w) = SCRIPT_WAKER.with(|w| w.borrow_mut().take()) {
        w.wake();
    }
}
fn reset() {
    log();
    SCRIPT.with(|s| s.borrow_mut().clear());
    SCRIPT_WAKER.with(|w| w.borrow_mut().take());
    RECOVER_WAKERS.with(|w| w.borrow_mut().clear());
}
/// Poll the driver until it is ready or a few polls went by: the script
/// call yields one request a poll.
fn run(d: &mut Driver<Q>) -> Poll<()> {
    let w = OUTER.with(Clone::clone);
    for _ in 0..16 {
        if Pin::new(&mut *d)
            .poll(&mut Context::from_waker(&w))
            .is_ready()
        {
            return Poll::Ready(());
        }
    }
    Poll::Pending
}

// ---------------------------------------------------------------- the structure

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathKey(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkId(pub u32);

pub struct Conn;
pub struct Established;
pub struct Entry;
pub struct LogicalPath;
pub struct LinkPath;
pub struct Recovery;

impl NodeKind for Conn {
    type Children = HList![
        Child<Established, AtMostOne>,
        Child<Entry, SlotVec<LinkId>>,
        Child<LogicalPath, SlotVec<PathKey>>,
    ];
    type Linked = HNil;
    type Request = ConnRequest;
}
/// Why the established layer ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EstablishedEnd {
    #[default]
    ConnectionClosed,
    ZeroRttRejected,
}
impl NodeKind for Established {
    type Children = HNil;
    type Linked = HNil;
    type EndReason = EstablishedEnd;
}
impl NodeKind for Entry {
    type Children = HNil;
    type Linked = HNil;
}
impl NodeKind for LogicalPath {
    type Children = HList![Child<LinkPath, SlotVec<LinkId>>];
    type Linked = HNil;
    type Request = PathRequest;
}
/// A link path links to the connection's entry of its own link.
impl NodeKind for LinkPath {
    type Children = HList![Child<Recovery, AtMostOne>];
    type Linked = HList![ByOwnKey<Entry>];
    type Request = LinkRequest;
}
impl nitori_driver::structure::LinkedStart<Entry> for LinkPath {
    fn linked_start(_: &mut ()) {}
}
impl NodeKind for Recovery {
    type Children = HNil;
    type Linked = HNil;
}

// ---------------------------------------------------------------- the requests, by kind

/// What the connection is asked.
pub enum ConnRequest {
    AddLogicalPath(PathKey),
    EndLogicalPath(PathKey),
    AddEstablished,
    EndEstablished,
    RebuildEstablished,
    EndConnection,
}
impl Request<Conn> for ConnRequest {
    fn apply<E: Editor>(self, mut edit: Edit<'_, E, Conn>) {
        let done = match self {
            Self::AddLogicalPath(k) => edit.add::<LogicalPath, _>(k, ()),
            Self::EndLogicalPath(k) => edit.end::<LogicalPath, _>(&k, ()),
            Self::AddEstablished => edit.add::<Established, _>((), ()),
            Self::EndEstablished => {
                edit.end::<Established, _>(&(), EstablishedEnd::ConnectionClosed)
            }
            Self::RebuildEstablished => {
                edit.replace::<Established, _>((), (), EstablishedEnd::ZeroRttRejected)
            }
            Self::EndConnection => {
                edit.end_self(());
                true
            }
        };
        if !done {
            put("refused");
        }
    }
}
/// What a logical path is asked.
pub enum PathRequest {
    AddLinkPath(LinkId),
    EndLinkPath(LinkId),
    MoveRecovery { from: LinkId, to: LinkId },
}
impl Request<LogicalPath> for PathRequest {
    fn apply<E: Editor>(self, mut edit: Edit<'_, E, LogicalPath>) {
        let done = match self {
            Self::AddLinkPath(l) => edit.add::<LinkPath, _>(l, ()),
            Self::EndLinkPath(l) => edit.end::<LinkPath, _>(&l, ()),
            Self::MoveRecovery { from, to } => edit
                .move_grandchild::<LinkPath, _, Recovery, _>(&from, &(), &to)
                .map_err(|e| put(format!("move: {e}")))
                .is_ok(),
        };
        if !done {
            put("refused");
        }
    }
}
/// What a link path is asked.
pub enum LinkRequest {
    AddRecovery,
    EndRecovery,
}
impl Request<LinkPath> for LinkRequest {
    fn apply<E: Editor>(self, mut edit: Edit<'_, E, LinkPath>) {
        let done = match self {
            Self::AddRecovery => edit.add::<Recovery, _>((), ()),
            Self::EndRecovery => edit.end::<Recovery, _>(&(), ()),
        };
        if !done {
            put("refused");
        }
    }
}

/// What the script call yields: requests for three kinds of node.
pub type Sent = Coprod!(
    NodeRequest<Conn, Conn>,
    NodeRequest<Conn, LogicalPath>,
    NodeRequest<Conn, LinkPath>,
);
fn to_conn(r: ConnRequest) -> Sent {
    Coproduct::Inl(NodeRequest::new(KeyAddress::here(), r))
}
fn path(k: u32) -> KeyAddress<Conn> {
    KeyAddress::child::<LogicalPath, _>(PathKey(k))
}
fn link(k: u32, l: u32) -> KeyAddress<Conn> {
    KeyAddress::below::<LogicalPath, _>(PathKey(k), KeyAddress::child::<LinkPath, _>(LinkId(l)))
}
fn to_path(k: u32, r: PathRequest) -> Sent {
    Coproduct::Inr(Coproduct::Inl(NodeRequest::new(path(k), r)))
}
fn to_link(k: u32, l: u32, r: LinkRequest) -> Sent {
    Coproduct::Inr(Coproduct::Inr(Coproduct::Inl(NodeRequest::new(
        link(k, l),
        r,
    ))))
}

// ---------------------------------------------------------------- components

/// A kind's name in the log.
pub trait Name {
    const NAME: &'static str;
}
impl Name for Conn {
    const NAME: &'static str = "Conn";
}
impl Name for Established {
    const NAME: &'static str = "Established";
}
impl Name for Entry {
    const NAME: &'static str = "Entry";
}
impl Name for LogicalPath {
    const NAME: &'static str = "LogicalPath";
}
impl Name for LinkPath {
    const NAME: &'static str = "LinkPath";
}
impl Name for Recovery {
    const NAME: &'static str = "Recovery";
}

/// Logs its node's build and end, with the reason.
pub struct Logged<K>(PhantomData<fn() -> K>);
impl<L: PartList<L> + 'static, K: Name + NodeKind<EndReason: Debug>> Hooks<Base, L, K>
    for Logged<K>
{
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, K>>>(
        _: BuildView<'_, Self, Base, L, K, KP>,
        _: &ArgsOf<L>,
        _: &mut StartOf<K>,
    ) -> Self
    where
        K: NodeKind,
    {
        put(format!("build {}", K::NAME));
        Logged(PhantomData)
    }
    fn end<KP: HookKeys<Self, Base, L, K>>(
        _: HookView<'_, Self, Base, L, K, KP>,
        reason: &EndReasonOf<K>,
    ) {
        put(format!("end {} {:?}", K::NAME, reason));
    }
}

/// On a recovery layer: a deadline the test sets; when it comes, the
/// recovery layer asks its link path, the first link of the first path,
/// to end it.
#[derive(Default)]
pub struct Timeout {
    pub at: Option<Duration>,
}
thread_local! {
    static TIMEOUT: RefCell<Option<Duration>> = const { RefCell::new(None) };
}
impl<L: PartList<L> + 'static> Hooks<Base, L, Recovery> for Timeout {
    type Timing = HasDeadline;
    type Returns = NodeRequest<Conn, LinkPath>;
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, Recovery>>>(
        _: BuildView<'_, Self, Base, L, Recovery, KP>,
        _: &ArgsOf<L>,
        _: &mut (),
    ) -> Self {
        Timeout {
            at: TIMEOUT.with(|t| t.borrow_mut().take()),
        }
    }
}
impl<L: PartList<L> + 'static> Timed<Base, L, Recovery> for Timeout {
    type Instant = Duration;
    fn deadline<KP: HookKeys<Self, Base, L, Recovery>>(
        view: &HookView<'_, Self, Base, L, Recovery, KP>,
    ) -> Option<Duration> {
        view.head.at
    }
    fn expire<KP: HookKeys<Self, Base, L, Recovery>>(
        view: HookView<'_, Self, Base, L, Recovery, KP>,
        _: Duration,
    ) -> Option<NodeRequest<Conn, LinkPath>> {
        view.head.at = None;
        put("timeout");
        Some(NodeRequest::new(link(1, 1), LinkRequest::EndRecovery))
    }
}

/// On a link path: reads its entry before the calls; the lend finds the
/// entry, or the structure's invariant is broken.
pub struct SeesEntry;
impl<L: PartList<L> + 'static> Hooks<Base, L, LinkPath> for SeesEntry {
    type Needs = HList![At<Kind<Conn>, ToChild<Entry, MyLink>, Ref<Logged<Entry>>>];
    const ACTS_BEFORE_CALLS: bool = true;
    fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, LinkPath>>>(
        _: BuildView<'_, Self, Base, L, LinkPath, KP>,
        _: &ArgsOf<L>,
        _: &mut (),
    ) -> Self {
        SeesEntry
    }
    fn before_calls<KP: HookKeys<Self, Base, L, LinkPath>>(
        _: HookView<'_, Self, Base, L, LinkPath, KP>,
        _: &Waker,
    ) -> Option<Self::Returns> {
        None
    }
}

// ---------------------------------------------------------------- the part

pub struct Base;
impl<L> Part<L> for Base {
    type Args = ();
}
impl Share<Conn> for Base {
    type Data = HList![Logged<Conn>];
}
impl Share<Established> for Base {
    type Data = HList![Logged<Established>];
}
impl Share<Entry> for Base {
    type Data = HList![Logged<Entry>];
}
impl Share<LogicalPath> for Base {
    type Data = HList![Logged<LogicalPath>];
}
impl Share<LinkPath> for Base {
    type Data = HList![Logged<LinkPath>, SeesEntry];
}
impl Share<Recovery> for Base {
    type Data = HList![Logged<Recovery>, Timeout];
}
impl ShareCalls<Conn> for Base {
    type Calls = HList![Script];
}
impl ShareCalls<Established> for Base {
    type Calls = HList![Handshake];
}
impl ShareCalls<Entry> for Base {
    type Calls = HNil;
}
impl ShareCalls<LogicalPath> for Base {
    type Calls = HNil;
}
impl ShareCalls<LinkPath> for Base {
    type Calls = HNil;
}
impl ShareCalls<Recovery> for Base {
    type Calls = HList![Recover];
}

// ---------------------------------------------------------------- calls

/// On the connection: yields what the test scripts, one at a time.
pub struct Script;
impl<Q: Composition> CallKind<Q> for Script {
    type Needs = HNil;
    type Op = Scripting;
    type Yield = Sent;
    fn start() -> Scripting {
        Scripting::new()
    }
}
#[call(yields = Sent)]
pub async fn scripting<W: CallFamily<C = Script>>(host: &mut impl LentAccess<Family = W>) {
    let _ = host;
    loop {
        let next = poll_fn(|cx| match SCRIPT.with(|s| s.borrow_mut().pop_front()) {
            Some(r) => Poll::Ready(r),
            None => {
                SCRIPT_WAKER.with(|w| *w.borrow_mut() = Some(cx.waker().clone()));
                Poll::Pending
            }
        })
        .await;
        yield next;
    }
}

/// On the established layer: logs each poll.
pub struct Handshake;
impl<Q: Composition> CallKind<Q> for Handshake {
    type Needs = HNil;
    type Op = Handshaking;
    fn start() -> Handshaking {
        Handshaking::new()
    }
}
#[call]
pub async fn handshaking<W: CallFamily<C = Handshake>>(host: &mut impl LentAccess<Family = W>) {
    let _ = host;
    poll_fn(|_| {
        put("handshake polled");
        Poll::<()>::Pending
    })
    .await;
}

/// On a recovery layer: counts its polls, so that a moved layer shows it
/// went on; it reads the instant, as a call lent by a driver can.
pub struct Recover;
impl<Q: Composition> CallKind<Q> for Recover {
    type Needs = HList![At<Up<0>, Here, Now<Duration>>];
    type Op = Recovering;
    fn start() -> Recovering {
        Recovering::new()
    }
}
#[call]
pub async fn recovering<W: CallFamily<C = Recover>>(host: &mut impl LentAccess<Family = W>) {
    let mut polls = 0;
    poll_fn(|cx| {
        let now: Duration = host.with(|mut view| view.needs().head);
        assert!(now < Duration::from_secs(1000));
        polls += 1;
        put(format!("recover {polls}"));
        RECOVER_WAKERS.with(|w| w.borrow_mut().push(cx.waker().clone()));
        Poll::<()>::Pending
    })
    .await;
}

pub struct Q;
composition!(impl driver for Q: Conn, HList![Base], TestClock);

fn driver(clock: TestClock) -> Driver<Q> {
    let d = Driver::<Q>::new(clock, frunk_core::hlist![()], ()).unwrap();
    assert_eq!(log(), ["build Conn"]);
    d
}

/// The links of the connection's entries.
fn entries(d: &Driver<Q>) -> Vec<u32> {
    let entries = &d.root().unwrap().children.tail.head;
    let mut out: Vec<u32> = entries
        .handles()
        .map(|h| entries.key(h).unwrap().0)
        .collect();
    out.sort();
    out
}

/// The recovery layers of the link paths of the logical path `k`, as
/// `(link, has one)`.
fn recoveries(d: &Driver<Q>, k: u32) -> Vec<(u32, bool)> {
    let root = d.root().unwrap();
    let paths = &root.children.tail.tail.head;
    let Some(h) = paths.find(&PathKey(k)) else {
        return Vec::new();
    };
    let links = &paths.get(h).unwrap().children.head;
    let mut out: Vec<(u32, bool)> = links
        .handles()
        .map(|h| {
            (
                links.key(h).unwrap().0,
                links.get(h).unwrap().children.head.is_some(),
            )
        })
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------- the table of the requests

#[test]
fn the_connection_adds_and_ends_logical_paths() {
    reset();
    let mut d = driver(TestClock::default());
    script([
        to_conn(ConnRequest::AddLogicalPath(PathKey(1))),
        to_conn(ConnRequest::AddLogicalPath(PathKey(1))),
    ]);
    let _ = run(&mut d);
    assert_eq!(log(), ["build LogicalPath", "refused"]);
    script([
        to_path(1, PathRequest::AddLinkPath(LinkId(7))),
        to_conn(ConnRequest::EndLogicalPath(PathKey(1))),
        to_conn(ConnRequest::EndLogicalPath(PathKey(1))),
    ]);
    let _ = run(&mut d);
    // the link path's entry is built before it and ends after it, once no
    // link path links to it
    assert_eq!(
        log(),
        [
            "build Entry",
            "build LinkPath",
            "end LinkPath ()",
            "end LogicalPath ()",
            "end Entry ()",
            "refused"
        ]
    );
}

#[test]
fn a_logical_path_adds_and_ends_its_link_paths() {
    reset();
    let mut d = driver(TestClock::default());
    script([
        to_conn(ConnRequest::AddLogicalPath(PathKey(1))),
        to_path(1, PathRequest::AddLinkPath(LinkId(1))),
        to_path(1, PathRequest::AddLinkPath(LinkId(2))),
    ]);
    let _ = run(&mut d);
    assert_eq!(
        log(),
        [
            "build LogicalPath",
            "build Entry",
            "build LinkPath",
            "build Entry",
            "build LinkPath"
        ]
    );
    assert_eq!(recoveries(&d, 1), [(1, false), (2, false)]);
    script([to_path(1, PathRequest::EndLinkPath(LinkId(1)))]);
    let _ = run(&mut d);
    assert_eq!(log(), ["end LinkPath ()", "end Entry ()"]);
    assert_eq!(recoveries(&d, 1), [(2, false)]);
}

#[test]
fn an_entry_starts_with_the_first_link_path_over_it_and_ends_with_the_last() {
    reset();
    let mut d = driver(TestClock::default());
    script([
        to_conn(ConnRequest::AddLogicalPath(PathKey(1))),
        to_conn(ConnRequest::AddLogicalPath(PathKey(2))),
        to_path(1, PathRequest::AddLinkPath(LinkId(1))),
        to_path(2, PathRequest::AddLinkPath(LinkId(1))),
    ]);
    let _ = run(&mut d);
    // one entry for both: the second link path finds it
    assert_eq!(
        log(),
        [
            "build LogicalPath",
            "build LogicalPath",
            "build Entry",
            "build LinkPath",
            "build LinkPath"
        ]
    );
    assert_eq!(entries(&d), [1]);
    script([to_path(1, PathRequest::EndLinkPath(LinkId(1)))]);
    let _ = run(&mut d);
    assert_eq!(log(), ["end LinkPath ()"]);
    assert_eq!(entries(&d), [1]);
    // ending the logical path ends its link path, the last over the entry
    script([to_conn(ConnRequest::EndLogicalPath(PathKey(2)))]);
    let _ = run(&mut d);
    assert_eq!(
        log(),
        ["end LinkPath ()", "end LogicalPath ()", "end Entry ()"]
    );
    assert!(entries(&d).is_empty());
}

#[test]
fn the_connection_adds_ends_and_replaces_its_established_layer() {
    reset();
    let mut d = driver(TestClock::default());
    script([to_conn(ConnRequest::AddEstablished)]);
    let _ = run(&mut d);
    assert_eq!(log(), ["build Established", "handshake polled"]);
    // replaced: the old layer's end, told why, then the new layer, with no
    // call polled in between; the new layer's call is polled after
    script([to_conn(ConnRequest::RebuildEstablished)]);
    let _ = run(&mut d);
    assert_eq!(
        log(),
        [
            "end Established ZeroRttRejected",
            "build Established",
            "handshake polled"
        ]
    );
    script([
        to_conn(ConnRequest::EndEstablished),
        to_conn(ConnRequest::RebuildEstablished),
    ]);
    let _ = run(&mut d);
    // without a layer, replacing builds none
    assert_eq!(log(), ["end Established ConnectionClosed", "refused"]);
}

#[test]
fn a_link_path_adds_and_ends_its_recovery_layer() {
    reset();
    let mut d = driver(TestClock::default());
    script([
        to_conn(ConnRequest::AddLogicalPath(PathKey(1))),
        to_path(1, PathRequest::AddLinkPath(LinkId(1))),
        to_link(1, 1, LinkRequest::AddRecovery),
        to_link(1, 1, LinkRequest::AddRecovery),
    ]);
    let _ = run(&mut d);
    assert_eq!(
        log(),
        [
            "build LogicalPath",
            "build Entry",
            "build LinkPath",
            "build Recovery",
            "refused",
            "recover 1"
        ]
    );
    assert_eq!(recoveries(&d, 1), [(1, true)]);
    script([to_link(1, 1, LinkRequest::EndRecovery)]);
    let _ = run(&mut d);
    assert_eq!(log(), ["end Recovery ()"]);
    assert_eq!(recoveries(&d, 1), [(1, false)]);
}

#[test]
fn a_logical_path_moves_a_recovery_layer_between_its_link_paths() {
    reset();
    let mut d = driver(TestClock::default());
    script([
        to_conn(ConnRequest::AddLogicalPath(PathKey(1))),
        to_conn(ConnRequest::AddLogicalPath(PathKey(2))),
        to_path(1, PathRequest::AddLinkPath(LinkId(1))),
        to_path(1, PathRequest::AddLinkPath(LinkId(2))),
        to_path(2, PathRequest::AddLinkPath(LinkId(3))),
        to_link(1, 1, LinkRequest::AddRecovery),
    ]);
    let _ = run(&mut d);
    assert_eq!(log().last().unwrap(), "recover 1");
    script([to_path(
        1,
        PathRequest::MoveRecovery {
            from: LinkId(1),
            to: LinkId(2),
        },
    )]);
    let _ = run(&mut d);
    // moved by value: not ended, not built again; its call goes on and is
    // polled where it went
    assert_eq!(log(), ["recover 2"]);
    assert_eq!(recoveries(&d, 1), [(1, false), (2, true)]);
    // the link path it is on now has one; a link path with none moves
    // nothing; one of another logical path is not in this one's subtree
    script([
        to_link(1, 2, LinkRequest::AddRecovery),
        to_path(
            1,
            PathRequest::MoveRecovery {
                from: LinkId(1),
                to: LinkId(2),
            },
        ),
        to_path(
            1,
            PathRequest::MoveRecovery {
                from: LinkId(2),
                to: LinkId(3),
            },
        ),
    ]);
    let _ = run(&mut d);
    assert_eq!(
        log(),
        [
            "refused",
            "move: no node at the path to move",
            "refused",
            "move: no node at the new parent's path",
            "refused"
        ]
    );
    // the moved layer's own waker still reaches it
    RECOVER_WAKERS.with(|w| w.borrow().last().unwrap().wake_by_ref());
    let _ = run(&mut d);
    assert_eq!(log(), ["recover 3"]);
}

#[test]
fn the_connection_ends_itself() {
    reset();
    let mut d = driver(TestClock::default());
    script([
        to_conn(ConnRequest::AddEstablished),
        to_conn(ConnRequest::AddLogicalPath(PathKey(1))),
        to_path(1, PathRequest::AddLinkPath(LinkId(1))),
        to_link(1, 1, LinkRequest::AddRecovery),
    ]);
    let _ = run(&mut d);
    log();
    script([
        to_conn(ConnRequest::EndConnection),
        to_conn(ConnRequest::AddEstablished),
    ]);
    assert_eq!(run(&mut d), Poll::Ready(()));
    assert!(d.ended());
    assert_eq!(
        log(),
        [
            "end Established ConnectionClosed",
            "end Recovery ()",
            "end LinkPath ()",
            "end LogicalPath ()",
            "end Entry ()",
            "end Conn ()"
        ]
    );
}

#[test]
fn a_request_for_a_node_not_there_is_dropped() {
    reset();
    let mut d = driver(TestClock::default());
    script([
        to_path(9, PathRequest::AddLinkPath(LinkId(1))),
        to_link(1, 1, LinkRequest::AddRecovery),
        to_conn(ConnRequest::AddLogicalPath(PathKey(1))),
    ]);
    let _ = run(&mut d);
    assert_eq!(log(), ["build LogicalPath"]);
}

#[test]
fn a_hook_returns_a_request() {
    reset();
    let clock = TestClock::default();
    let mut d = driver(clock.clone());
    TIMEOUT.with(|t| *t.borrow_mut() = Some(Duration::from_secs(10)));
    script([
        to_conn(ConnRequest::AddLogicalPath(PathKey(1))),
        to_path(1, PathRequest::AddLinkPath(LinkId(1))),
        to_link(1, 1, LinkRequest::AddRecovery),
    ]);
    let _ = run(&mut d);
    log();
    clock.set(Duration::from_secs(10));
    let _ = run(&mut d);
    assert_eq!(log(), ["timeout", "end Recovery ()"]);
    assert_eq!(recoveries(&d, 1), [(1, false)]);
}

// ---------------------------------------------------------------- an HTTP-like structure

/// A connection with request streams by ID, each with a body (at most
/// one); the connection closes a stream when asked, and a body's call asks
/// it once the body is drained.
mod http_like {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct StreamId(pub u64);

    pub struct H;
    pub struct Stream;
    pub struct Body;
    impl NodeKind for H {
        type Children = HList![Child<Stream, SlotVec<StreamId>>];
        type Linked = HNil;
        type Request = CloseStream;
    }
    impl NodeKind for Stream {
        type Children = HList![Child<Body, AtMostOne>];
        type Linked = HNil;
    }
    impl NodeKind for Body {
        type Children = HNil;
        type Linked = HNil;
    }
    impl Name for H {
        const NAME: &'static str = "H";
    }
    impl Name for Stream {
        const NAME: &'static str = "Stream";
    }
    impl Name for Body {
        const NAME: &'static str = "Body";
    }

    /// Close the stream of this ID.
    pub struct CloseStream(pub StreamId);
    impl Request<H> for CloseStream {
        fn apply<E: Editor>(self, mut edit: Edit<'_, E, H>) {
            edit.end::<Stream, _>(&self.0, ());
        }
    }

    impl Share<H> for Base {
        type Data = HList![Logged<H>, Seen];
    }

    /// On the connection: the instant a stream started at, and the instant
    /// of each action before the calls.
    pub struct Seen;
    impl<L: PartList<L> + 'static> Hooks<Base, L, H> for Seen {
        type Needs = HList![At<Up<0>, Here, Now<Duration>>];
        type Subscribes = HList![Hear<NodeStarted<Stream>>];
        const ACTS_BEFORE_CALLS: bool = true;
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, H>>>(
            _: BuildView<'_, Self, Base, L, H, KP>,
            _: &ArgsOf<L>,
            _: &mut (),
        ) -> Self {
            Seen
        }
        fn before_calls<KP: HookKeys<Self, Base, L, H>>(
            view: HookView<'_, Self, Base, L, H, KP>,
            _: &Waker,
        ) -> Option<Self::Returns> {
            put(format!("before the calls at {}", view.tail.head.as_secs()));
            None
        }
        fn on_events<KP: HookKeys<Self, Base, L, H>>(
            _: Triggered<'_>,
            view: HookView<'_, Self, Base, L, H, KP>,
        ) -> Option<Self::Returns> {
            put(format!("stream started at {}", view.tail.head.as_secs()));
            None
        }
    }
    impl Share<Stream> for Base {
        type Data = HList![Logged<Stream>];
    }
    impl Share<Body> for Base {
        type Data = HList![Logged<Body>];
    }
    impl ShareCalls<H> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Stream> for Base {
        type Calls = HNil;
    }
    impl ShareCalls<Body> for Base {
        type Calls = HList![Drain];
    }

    /// On a body: drained at once, it asks the connection to close the
    /// fourth stream, its own.
    pub struct Drain;
    impl<Q: Composition> CallKind<Q> for Drain {
        type Needs = HNil;
        type Op = Draining;
        type Yield = NodeRequest<H, H>;
        fn start() -> Draining {
            Draining::new()
        }
    }
    #[call(yields = NodeRequest<H, H>)]
    pub async fn draining<W: CallFamily<C = Drain>>(host: &mut impl LentAccess<Family = W>) {
        let _ = host;
        put("drained");
        yield NodeRequest::new(KeyAddress::here(), CloseStream(StreamId(4)));
        put("not polled again");
    }

    pub struct QH;
    composition!(impl driver for QH: H, HList![Base], TestClock);

    #[test]
    fn a_body_asks_the_connection_to_close_its_stream() {
        reset();
        let clock = TestClock::default();
        let mut d = Driver::<QH>::new(clock.clone(), frunk_core::hlist![()], ()).unwrap();
        clock.set(Duration::from_secs(2));
        let stream = d
            .add::<Stream, _, _>(nitori_driver::structure::End, StreamId(4), ())
            .unwrap();
        d.add::<Body, _, _>(stream, (), ()).unwrap();
        // the hooks read the instant the driver read when it was entered
        assert_eq!(
            log(),
            [
                "build H",
                "build Stream",
                "stream started at 2",
                "build Body"
            ]
        );
        clock.set(Duration::from_secs(5));
        let w = OUTER.with(Clone::clone);
        let _ = Pin::new(&mut d).poll(&mut Context::from_waker(&w));
        let _ = Pin::new(&mut d).poll(&mut Context::from_waker(&w));
        assert_eq!(
            log(),
            [
                "before the calls at 5",
                "drained",
                "end Body ()",
                "end Stream ()"
            ]
        );
        assert!(d.root().unwrap().children.head.handles().next().is_none());
    }
}

//! The driver on a connection-like structure: hooks lent views, deadlines,
//! events, calls woken and polled, nodes added and removed, the driver
//! dropped and panicking.
#![feature(impl_trait_in_assoc_type, yield_expr)]
mod common;

use core::{future::poll_fn, pin::Pin, task::Poll};
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Wake, Waker},
    time::Duration,
};

use common::{clock::TestClock, net::*};
use frunk_core::{HList, hlist::HNil, hlist_pat};
use nitori_call::{LentAccess, call};
use nitori_driver::{
    call::{CallFamily, CallKind, Composition, LoanNeeds, Positions, ShareCalls, View},
    composition,
    driver::Driver,
    event::{Event, ForDeadline, Hear, HearAt, NodeEnding, NodeStarted, Publish, Triggered},
    hooks::{
        AbortCause, ArgsOf, BuildKeyed, BuildView, EndReasonOf, HasDeadline, HookKeys, HookView,
        Hooks, PartList, Timed,
    },
    lend::KeyedList,
    need::{At, ByKey, Here, Kind, Mut, Ref, ToChild, Up},
    request::NodeRequest,
    structure::{End, KeyAddress, NodeKind, Share},
};

// ---------------------------------------------------------------- a log

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static WAKERS: RefCell<Vec<(&'static str, Waker)>> = const { RefCell::new(Vec::new()) };
}
fn put(what: impl Into<String>) {
    LOG.with(|l| l.borrow_mut().push(what.into()));
}
fn log() -> Vec<String> {
    LOG.with(|l| core::mem::take(&mut *l.borrow_mut()))
}
fn stash(name: &'static str, w: &Waker) {
    WAKERS.with(|s| s.borrow_mut().push((name, w.clone())));
}
fn stashed(name: &'static str) -> Vec<Waker> {
    WAKERS.with(|s| {
        s.borrow()
            .iter()
            .filter(|(n, _)| *n == name)
            .map(|(_, w)| w.clone())
            .collect()
    })
}
fn clear_stash() {
    WAKERS.with(|s| s.borrow_mut().clear());
}

/// An outer waker that counts its wakes.
#[derive(Default)]
struct Counting(AtomicUsize);
impl Wake for Counting {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn outer() -> (Arc<Counting>, Waker) {
    let c = Arc::new(Counting::default());
    (c.clone(), Waker::from(c))
}
fn poll<Q: nitori_driver::driver::DriverComposition>(d: &mut Driver<Q>, w: &Waker) -> Poll<()>
where
    Driver<Q>: core::future::Future<Output = ()>,
    nitori_driver::driver::walk::Cursor<frunk_core::indices::Here>:
        nitori_driver::driver::walk::CallWalk<Q>,
{
    Pin::new(d).poll(&mut Context::from_waker(w))
}

// ---------------------------------------------------------------- events

/// A path's count went up, at the path.
pub struct Bumped;
impl Event for Bumped {}
/// The connection's total changed, at the connection.
pub struct TotalChanged;
impl Event for TotalChanged {}
/// A path's timer expired, published at the connection.
pub struct Expired;
impl Event for Expired {}

// ---------------------------------------------------------------- components

macro_rules! build_default {
    () => {
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, K_>>>(
            _: BuildView<'_, Self, Base, L, K_, KP>,
            _: &ArgsOf<L>,
        ) -> Self {
            Default::default()
        }
    };
}
/// Every kind's end is logged by one component of it.
macro_rules! end_logged {
    ($name:literal) => {
        fn end<KP: HookKeys<Self, Base, L, K_>>(
            _: HookView<'_, Self, Base, L, K_, KP>,
            _: &EndReasonOf<K_>,
        ) {
            put(concat!("end ", $name));
        }
        fn aborted<KP: HookKeys<Self, Base, L, K_>>(
            _: HookView<'_, Self, Base, L, K_, KP>,
            cause: AbortCause,
        ) {
            put(format!("aborted {} {:?}", $name, cause));
        }
    };
}

/// On the connection: a total; the part keeps it after the end.
#[derive(Debug, Default)]
pub struct Total(pub u64);
type K_ = Connection;
impl<L: PartList<L> + 'static> Hooks<Base, L, Connection> for Total {
    build_default!();
    end_logged!("Connection");
    fn remains<KP: HookKeys<Self, Base, L, K_>>(
        view: HookView<'_, Self, Base, L, K_, KP>,
        out: &mut u64,
    ) {
        put("remains");
        *out = view.head.0;
    }
}

/// On the session: acts before the calls, counting how often.
#[derive(Debug, Default)]
pub struct SessionState(pub u64);
mod session {
    use super::*;
    type K_ = Session;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Session> for SessionState {
        const ACTS_BEFORE_CALLS: bool = true;
        build_default!();
        end_logged!("Session");
        fn before_calls<KP: HookKeys<Self, Base, L, K_>>(
            view: HookView<'_, Self, Base, L, K_, KP>,
            waker: &Waker,
        ) -> Option<Self::Returns> {
            view.head.0 += 1;
            put(format!("before {}", view.head.0));
            stash("session", waker);
            None
        }
    }
}

/// On a peer: a deadline the connection's total moves, read again on
/// `TotalChanged`; it hears `Bumped` from every path.
#[derive(Debug, Default)]
pub struct PeerState {
    pub bumped: u64,
}
mod peer {
    use super::*;
    type K_ = Peer;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Peer> for PeerState {
        type Needs = HList![At<Kind<Connection>, Here, Ref<Total>>];
        type Subscribes = HList![
            ForDeadline<TotalChanged>,
            HearAt<Bumped, Kind<Connection>, ToChild<Path, ByKey<PathId>>>,
        ];
        type Timing = HasDeadline;
        build_default!();
        end_logged!("Peer");
        fn on_events<KP: HookKeys<Self, Base, L, K_>>(
            events: Triggered<'_>,
            view: HookView<'_, Self, Base, L, K_, KP>,
        ) -> Option<Self::Returns> {
            assert!(events.contains::<Bumped>());
            view.head.bumped += 1;
            None
        }
    }
    impl<L: PartList<L> + 'static> Timed<Base, L, Peer> for PeerState {
        type Instant = Duration;
        fn deadline<KP: HookKeys<Self, Base, L, K_>>(
            view: &HookView<'_, Self, Base, L, K_, KP>,
        ) -> Option<Duration> {
            let total = view.tail.head.0;
            (total > 0).then(|| Duration::from_secs(100 + total))
        }
        fn expire<KP: HookKeys<Self, Base, L, K_>>(
            _: HookView<'_, Self, Base, L, K_, KP>,
            _: Duration,
        ) -> Option<Self::Returns> {
            None
        }
    }
}

/// On a path: a count built from the connection's total.
#[derive(Debug, Default)]
pub struct PathCount(pub u64);
/// On a path: a timer the path's call arms.
#[derive(Debug, Default)]
pub struct PathTimer {
    pub at: Option<Duration>,
    pub fired: u64,
}
mod path {
    use super::*;
    type K_ = Path;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Path> for PathCount {
        type BuildNeeds = HList![At<Up<0>, Here, Ref<Total>>];
        fn build<KP: KeyedList<L, BuildKeyed<Self, Base, L, K_>>>(
            parents: BuildView<'_, Self, Base, L, K_, KP>,
            _: &ArgsOf<L>,
        ) -> Self {
            let hlist_pat![total] = parents;
            PathCount(total.0 * 10)
        }
        end_logged!("Path");
    }
    impl<L: PartList<L> + 'static> Hooks<Base, L, Path> for PathTimer {
        type Needs = HList![At<Kind<Connection>, Here, Publish<Expired>>];
        type Timing = HasDeadline;
        build_default!();
    }
    impl<L: PartList<L> + 'static> Timed<Base, L, Path> for PathTimer {
        type Instant = Duration;
        fn deadline<KP: HookKeys<Self, Base, L, K_>>(
            view: &HookView<'_, Self, Base, L, K_, KP>,
        ) -> Option<Duration> {
            view.head.at
        }
        fn expire<KP: HookKeys<Self, Base, L, K_>>(
            view: HookView<'_, Self, Base, L, K_, KP>,
            now: Duration,
        ) -> Option<Self::Returns> {
            let hlist_pat![this, expired] = view;
            this.fired += 1;
            this.at = None;
            put(format!("expired at {}", now.as_secs()));
            expired.publish();
            None
        }
    }
}

#[derive(Debug, Default)]
pub struct RouteState;
#[derive(Debug, Default)]
pub struct TimerState;
mod route {
    use super::*;
    type K_ = Route;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Route> for RouteState {
        build_default!();
        end_logged!("Route");
    }
}
mod timer {
    use super::*;
    type K_ = Timer;
    impl<L: PartList<L> + 'static> Hooks<Base, L, Timer> for TimerState {
        build_default!();
        end_logged!("Timer");
    }
}

/// On the connection, in the second part: what it heard.
#[derive(Debug, Default)]
pub struct Heard {
    pub started: u64,
    pub ending: u64,
    pub bumped: u64,
    pub expired: u64,
}
impl<L: PartList<L> + 'static> Hooks<Watch, L, Connection> for Heard {
    type Subscribes = HList![
        Hear<NodeStarted<Path>>,
        Hear<NodeEnding<Path>>,
        Hear<Bumped>,
        Hear<Expired>,
    ];
    fn build<KP: KeyedList<L, BuildKeyed<Self, Watch, L, Connection>>>(
        _: BuildView<'_, Self, Watch, L, Connection, KP>,
        _: &ArgsOf<L>,
    ) -> Self {
        Default::default()
    }
    fn on_events<KP: HookKeys<Self, Watch, L, Connection>>(
        events: Triggered<'_>,
        view: HookView<'_, Self, Watch, L, Connection, KP>,
    ) -> Option<Self::Returns> {
        let this = view.head;
        this.started += events.contains::<NodeStarted<Path>>() as u64;
        this.ending += events.contains::<NodeEnding<Path>>() as u64;
        this.bumped += events.contains::<Bumped>() as u64;
        this.expired += events.contains::<Expired>() as u64;
        None
    }
}

// ---------------------------------------------------------------- parts

pub struct Base;
impl Share<Connection> for Base {
    type Data = HList![Total];
    type Remains = u64;
}
impl Share<Session> for Base {
    type Data = HList![SessionState];
}
impl Share<Peer> for Base {
    type Data = HList![PeerState];
}
impl Share<Path> for Base {
    type Data = HList![PathCount, PathTimer];
}
impl Share<Route> for Base {
    type Data = HList![RouteState];
}
impl Share<Timer> for Base {
    type Data = HList![TimerState];
}
pub struct Watch;
impl Share<Connection> for Watch {
    type Data = HList![Heard];
}
macro_rules! empty_share {
    ($part:ty: $($k:ty),*) => {$(
        impl Share<$k> for $part {
            type Data = HNil;
        }
        impl ShareCalls<$k> for $part {
            type Calls = HNil;
        }
    )*};
}
empty_share!(Watch: Session, Peer, Path, Route, Timer);
impl ShareCalls<Connection> for Watch {
    type Calls = HNil;
}
impl<L> nitori_driver::hooks::Part<L> for Base {
    type Args = ();
}
impl<L> nitori_driver::hooks::Part<L> for Watch {
    type Args = ();
}
impl ShareCalls<Connection> for Base {
    type Calls = HList![Advance];
}
impl ShareCalls<Session> for Base {
    type Calls = HList![Idle];
}
impl ShareCalls<Peer> for Base {
    type Calls = HNil;
}
impl ShareCalls<Path> for Base {
    type Calls = HList![Bump];
}
impl ShareCalls<Route> for Base {
    type Calls = HNil;
}
impl ShareCalls<Timer> for Base {
    type Calls = HNil;
}

// ---------------------------------------------------------------- calls

/// On the connection: the total up by one, `TotalChanged` published,
/// the total yielded; over at three.
pub struct Advance;
impl<Q: Composition> CallKind<Q> for Advance {
    type Needs = HList![
        At<Up<0>, Here, Mut<Total>>,
        At<Up<0>, Here, Publish<TotalChanged>>
    ];
    type Op = Advancing;
    type Yield = NodeRequest<Connection, Connection>;
    fn start() -> Advancing {
        Advancing::new()
    }
}
#[call(yields = NodeRequest<Connection, Connection>)]
pub async fn advancing<W: CallFamily<C = Advance>>(host: &mut impl LentAccess<Family = W>) {
    loop {
        let total = host.with(|mut view| {
            let hlist_pat![total, changed] = view.needs();
            total.0 += 1;
            changed.publish();
            total.0
        });
        put(format!("advance {total}"));
        yield NodeRequest::new(KeyAddress::here(), Noted(total));
        if total == 3 {
            return;
        }
    }
}

/// On the session: waits forever; no longer needed once the session
/// acted before the calls twice.
pub struct Idle;
impl<Q: Composition> CallKind<Q> for Idle {
    type Needs = HList![At<Up<0>, Here, Ref<SessionState>>];
    type Op = Idling;
    fn start() -> Idling {
        Idling::new()
    }
    fn no_longer_needed<KP: Positions<Q, Self>>(view: &View<'_, Self, Q, KP>) -> bool {
        view.head.0 >= 2
    }
}
#[call]
pub async fn idling<W: CallFamily<C = Idle>>(host: &mut impl LentAccess<Family = W>) {
    let _ = host;
    poll_fn(|cx| {
        put("idle");
        stash("idle", cx.waker());
        Poll::<()>::Pending
    })
    .await;
}

/// On a path: once, the count up, `Bumped` published at the path, the
/// timer armed at five seconds; then waits, its waker kept.
pub struct Bump;
impl<Q: Composition> CallKind<Q> for Bump {
    type Needs = HList![
        At<Up<0>, Here, Mut<PathCount>>,
        At<Up<0>, Here, Mut<PathTimer>>,
        At<Up<0>, Here, Publish<Bumped>>,
    ];
    type Op = Bumping;
    fn start() -> Bumping {
        Bumping::new()
    }
}
#[call]
pub async fn bumping<W: CallFamily<C = Bump>>(host: &mut impl LentAccess<Family = W>) {
    host.with(|mut view| {
        let hlist_pat![count, timer, bumped] = view.needs();
        count.0 += 1;
        timer.at = Some(Duration::from_secs(5));
        bumped.publish();
        put(format!("bump {}", count.0));
    });
    poll_fn(|cx| {
        put("bump waits");
        stash("bump", cx.waker());
        Poll::<()>::Pending
    })
    .await;
}

pub struct Q;
composition!(impl driver for Q: Connection, HList![Base, Watch], TestClock);

type Args = ArgsOf<HList![Base, Watch]>;
fn args() -> Args {
    frunk_core::hlist![(), ()]
}
fn heard(d: &Driver<Q>) -> (u64, u64, u64, u64) {
    let h = &d.root().unwrap().data.tail.head.head;
    (h.started, h.ending, h.bumped, h.expired)
}

// ---------------------------------------------------------------- tests

#[test]
fn calls_run_yield_and_finish() {
    log();
    let clock = TestClock::default();
    let mut d = Driver::<Q>::new(clock, args()).unwrap();
    let (count, w) = outer();
    assert_eq!(poll(&mut d, &w), Poll::Pending);
    assert_eq!(log(), ["advance 1"]);
    // a call that yields is polled again: it marked itself, waking the
    // outer waker once
    assert_eq!(count.0.load(Ordering::SeqCst), 1);
    assert_eq!(poll(&mut d, &w), Poll::Pending);
    assert_eq!(poll(&mut d, &w), Poll::Pending);
    assert_eq!(log(), ["advance 2", "advance 3"]);
    // what it yielded went to the connection, a request each
    assert_eq!(noted(), [1, 2, 3]);
    // over: not polled again
    assert_eq!(poll(&mut d, &w), Poll::Pending);
    assert!(log().is_empty());
    assert_eq!(d.root().unwrap().data.head.head.0, 3);
}

#[test]
fn build_reads_the_parent_and_events_reach_above_not_beside() {
    log();
    clear_stash();
    let mut d = Driver::<Q>::new(TestClock::default(), args()).unwrap();
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    let _ = noted();
    // the total is 1: a path is built with 10
    let p1 = d.add::<Path, _, _>(End, PathId(1)).unwrap();
    let p2 = d.add::<Path, _, _>(End, PathId(2)).unwrap();
    let peer = d.add::<Peer, _, _>(End, PeerId(7)).unwrap();
    assert_eq!(
        heard(&d).0,
        2,
        "NodeStarted<Path> twice, at the connection above"
    );
    log();
    let _ = poll(&mut d, &w);
    let l = log();
    assert!(l.contains(&"bump 11".to_string()), "{l:?}");
    let root = d.root().unwrap();
    use nitori_driver::structure::NodePath;
    assert_eq!(p1.node(root).unwrap().data.head.head.0, 11);
    assert_eq!(p2.node(root).unwrap().data.head.head.0, 11);
    // `Bumped` at each path: the connection above hears both, the peer
    // beside them hears them through its anchor and steps
    assert_eq!(heard(&d).2, 2);
    assert_eq!(peer.node(root).unwrap().data.head.head.bumped, 2);
}

#[test]
fn deadlines_come_and_are_read_again() {
    log();
    clear_stash();
    let clock = TestClock::default();
    let mut d = Driver::<Q>::new(clock.clone(), args()).unwrap();
    let (_, w) = outer();
    let _p = d.add::<Path, _, _>(End, PathId(1)).unwrap();
    let _peer = d.add::<Peer, _, _>(End, PeerId(1)).unwrap();
    // the call arms the timer: its write is read again (debug builds check
    // every deadline at the end of a poll, and the peer's moves with the
    // total, read again on `TotalChanged`)
    let _ = poll(&mut d, &w);
    log();
    clock.set(Duration::from_secs(4));
    let _ = poll(&mut d, &w);
    assert!(!log().iter().any(|l| l.starts_with("expired")));
    clock.set(Duration::from_secs(5));
    let _ = poll(&mut d, &w);
    let l = log();
    assert!(l.contains(&"expired at 5".to_string()), "{l:?}");
    assert_eq!(heard(&d).3, 1, "the expiry published at the connection");
    // gone after its expiry
    clock.set(Duration::from_secs(50));
    let _ = poll(&mut d, &w);
    assert!(!log().iter().any(|l| l.starts_with("expired")));
}

#[test]
fn removal_order_end_hooks_and_remains() {
    log();
    let mut d = Driver::<Q>::new(TestClock::default(), args()).unwrap();
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    d.add::<Session, _, _>(End, ()).unwrap();
    d.add::<Peer, _, _>(End, PeerId(1)).unwrap();
    let p = d.add::<Path, _, _>(End, PathId(1)).unwrap();
    let r = d.add::<Route, _, _>(p, PeerId(1)).unwrap();
    d.add::<Timer, _, _>(r, ()).unwrap();
    log();
    d.end(());
    assert!(d.ended());
    assert_eq!(
        log(),
        [
            "end Session",
            "end Timer",
            "end Route",
            "end Path",
            "end Peer",
            "remains",
            "end Connection"
        ]
    );
    assert_eq!(d.remains(), Some(&frunk_core::hlist![1u64, ()]));
    assert_eq!(poll(&mut d, &w), Poll::Ready(()));
}

#[test]
fn removing_a_node_ends_it_and_its_subtree() {
    log();
    let mut d = Driver::<Q>::new(TestClock::default(), args()).unwrap();
    d.add::<Peer, _, _>(End, PeerId(1)).unwrap();
    let p = d.add::<Path, _, _>(End, PathId(1)).unwrap();
    d.add::<Route, _, _>(p, PeerId(1)).unwrap();
    log();
    assert!(d.remove(p, ()));
    assert_eq!(log(), ["end Route", "end Path"]);
    assert_eq!(heard(&d).1, 1, "NodeEnding<Path> reaches the connection");
    assert!(!d.remove(p, ()));
}

#[test]
fn before_calls_and_no_longer_needed() {
    log();
    clear_stash();
    let mut d = Driver::<Q>::new(TestClock::default(), args()).unwrap();
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    let _ = noted();
    log();
    d.add::<Session, _, _>(End, ()).unwrap();
    let _ = poll(&mut d, &w);
    // the action before the calls runs first
    let l = log();
    let before = l.iter().position(|x| x == "before 1").unwrap();
    let idle = l.iter().position(|x| x == "idle").unwrap();
    assert!(before < idle, "{l:?}");
    // its own waker runs it again; the call is not woken
    stashed("session")[0].wake_by_ref();
    let _ = poll(&mut d, &w);
    assert!(log().iter().all(|x| x != "idle"));
    // the call woken now is no longer needed: dropped, not polled
    stashed("idle")[0].wake_by_ref();
    let _ = poll(&mut d, &w);
    assert!(log().is_empty());
    stashed("idle")[0].wake_by_ref();
    let _ = poll(&mut d, &w);
    assert!(log().is_empty());
}

#[test]
fn old_wakers_after_a_node_goes() {
    log();
    clear_stash();
    let mut d = Driver::<Q>::new(TestClock::default(), args()).unwrap();
    let (_, w) = outer();
    let p = d.add::<Path, _, _>(End, PathId(1)).unwrap();
    let _ = poll(&mut d, &w);
    log();
    let old = stashed("bump").pop().unwrap();
    assert!(d.remove(p, ()));
    // the old node's waker marks nothing
    old.wake_by_ref();
    let _ = poll(&mut d, &w);
    assert!(log().iter().all(|x| !x.starts_with("bump")));
    // a new node at the same place and key: its call starts; the old
    // waker does not call it
    let again = d.add::<Path, _, _>(End, PathId(1)).unwrap();
    assert_eq!(again, p);
    let _ = poll(&mut d, &w);
    assert!(log().contains(&"bump waits".to_string()));
    old.wake_by_ref();
    let _ = poll(&mut d, &w);
    assert!(log().is_empty());
    // its own waker does
    stashed("bump").pop().unwrap().wake_by_ref();
    let _ = poll(&mut d, &w);
    assert_eq!(log(), ["bump waits"]);
}

#[test]
fn many_wakes_wake_the_outer_waker_once() {
    log();
    clear_stash();
    let mut d = Driver::<Q>::new(TestClock::default(), args()).unwrap();
    for i in 0..3 {
        d.add::<Path, _, _>(End, PathId(i)).unwrap();
    }
    let (count, w) = outer();
    for _ in 0..4 {
        let _ = poll(&mut d, &w);
    }
    let _ = noted();
    log();
    let before = count.0.load(Ordering::SeqCst);
    for waker in stashed("bump") {
        waker.wake_by_ref();
    }
    assert_eq!(count.0.load(Ordering::SeqCst), before + 1);
    let _ = poll(&mut d, &w);
    assert_eq!(log().iter().filter(|x| *x == "bump waits").count(), 3);
}

#[test]
fn a_new_outer_waker_polls_every_call_again() {
    log();
    clear_stash();
    let mut d = Driver::<Q>::new(TestClock::default(), args()).unwrap();
    d.add::<Path, _, _>(End, PathId(1)).unwrap();
    let (_, w1) = outer();
    for _ in 0..4 {
        let _ = poll(&mut d, &w1);
    }
    log();
    let (count2, w2) = outer();
    let _ = poll(&mut d, &w2);
    assert_eq!(log(), ["bump waits"]);
    // the call's new waker wakes the new outer waker
    stashed("bump").pop().unwrap().wake_by_ref();
    assert_eq!(count2.0.load(Ordering::SeqCst), 1);
}

#[test]
fn dropped_tells_every_component_then_ends() {
    log();
    let mut d = Driver::<Q>::new(TestClock::default(), args()).unwrap();
    d.add::<Session, _, _>(End, ()).unwrap();
    let p = d.add::<Path, _, _>(End, PathId(1)).unwrap();
    let _ = p;
    log();
    drop(d);
    assert_eq!(
        log(),
        [
            "aborted Connection Dropped",
            "aborted Session Dropped",
            "aborted Path Dropped",
            "end Session",
            "end Path",
            "remains",
            "end Connection"
        ]
    );
}

// ---------------------------------------------------------------- a panic

/// A part with a call on the timer that panics.
pub struct Boomer;
empty_share!(Boomer: Connection, Session, Peer, Path, Route);
impl Share<Timer> for Boomer {
    type Data = HNil;
}
impl ShareCalls<Timer> for Boomer {
    type Calls = HList![Boom];
}
impl<L> nitori_driver::hooks::Part<L> for Boomer {
    type Args = ();
}
pub struct Boom;
impl<Q: Composition> CallKind<Q> for Boom {
    type Needs = HNil;
    type Op = Booming;
    fn start() -> Booming {
        Booming::new()
    }
}
#[call]
pub async fn booming<W: CallFamily<C = Boom>>(host: &mut impl LentAccess<Family = W>) {
    let _ = host;
    panic!("boom");
}
pub struct QB;
composition!(impl driver for QB: Connection, HList![Base, Watch, Boomer], TestClock);

#[test]
fn a_panic_tells_the_root_only() {
    log();
    let mut d = Driver::<QB>::new(TestClock::default(), frunk_core::hlist![(), (), ()]).unwrap();
    d.add::<Peer, _, _>(End, PeerId(1)).unwrap();
    let p = d.add::<Path, _, _>(End, PathId(1)).unwrap();
    let r = d.add::<Route, _, _>(p, PeerId(1)).unwrap();
    d.add::<Timer, _, _>(r, ()).unwrap();
    log();
    let (_, w) = outer();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poll(&mut d, &w)));
    assert!(unwound.is_err());
    log();
    drop(d);
    assert_eq!(log(), ["aborted Connection Panicked", "remains"]);
}

#[allow(dead_code)]
fn _kinds_have_default_reasons(_: <Path as NodeKind>::EndReason) {}

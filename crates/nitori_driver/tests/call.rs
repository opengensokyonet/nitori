//! Calls: `#[call]` functions generic over their family, listed in the
//! parts' shares, proved once by the composition, lent a new view and
//! polled several times.
#![feature(impl_trait_in_assoc_type, yield_expr)]
mod common;

use core::{future::poll_fn, pin::pin, task::Poll};
use std::{
    cell::RefCell,
    task::{Context, Waker},
};

use common::{net::*, new};
use frunk_core::{HList, hlist::Selector, hlist_pat};
use nitori_call::{LentAccess, call};
use nitori_driver::{
    call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
    composition,
    need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, MyLink, Ref, ToChild, Up},
    structure::{End, Node, NodePath, Share, insert},
};

thread_local! {
    static SEEN: RefCell<Vec<(&'static str, u64)>> = const { RefCell::new(Vec::new()) };
}
fn put(what: &'static str, v: u64) {
    SEEN.with(|l| l.borrow_mut().push((what, v)));
}
fn seen() -> Vec<(&'static str, u64)> {
    SEEN.with(|l| core::mem::take(&mut *l.borrow_mut()))
}

/// Pending once, waking itself, then ready: a call's way to wait for the
/// next poll.
fn next_poll() -> impl Future<Output = ()> {
    let mut first = true;
    poll_fn(move |cx| {
        if first {
            first = false;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
}

// ---------------------------------------------------------------- calls

/// On the connection: the clock, one tick a poll, three ticks, then over.
pub struct Advance;
impl<Q: Composition> CallKind<Q> for Advance {
    type Needs = HList![At<Up<0>, Here, Mut<Clock>>, At<Up<0>, Here, Ref<Keys>>];
    type Op = Advancing;
    fn start() -> Advancing {
        Advancing::new()
    }
}
#[call]
pub async fn advancing<W: CallFamily<C = Advance>>(host: &mut impl LentAccess<Family = W>) {
    let mut ticks = 0u64;
    loop {
        host.with(|mut view| {
            let hlist_pat![clock, keys] = view.needs();
            clock.0 += 1;
            put("advance", clock.0 * 1000 + keys.0);
        });
        ticks += 1;
        if ticks == 3 {
            return;
        }
        next_poll().await;
    }
}

/// On a path: acknowledgements, waited for across polls. One for another
/// path changes that path's state by its ID ("all but the one on the
/// way"); one for this path goes through `Up<0>`. The call counts the
/// batches across polls.
pub struct PathAckReceive;
impl<Q: Composition> CallKind<Q> for PathAckReceive {
    type Needs = HList![
        At<Up<0>, Here, Mut<PathState>>,
        At<Up<0>, Here, Ref<PathIdent>>,
        At<Up<0>, Here, Mut<PathAcks>>,
        At<Kind<Connection>, ToChild<Path, ByKeyOthers<PathId>>, Mut<PathState>>,
    ];
    type Op = PathAckReceiving;
    fn start() -> PathAckReceiving {
        PathAckReceiving::new()
    }
}
#[call]
pub async fn path_ack_receiving<W: CallFamily<C = PathAckReceive>>(
    host: &mut impl LentAccess<Family = W>,
) {
    let mut batches = 0u64;
    loop {
        poll_fn(|_| {
            host.with(|mut view| {
                let hlist_pat![_, _, acks, _] = view.needs();
                if acks.0.is_empty() {
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
        })
        .await;
        batches += 1;
        host.with(|mut view| {
            let hlist_pat![own, ident, acks, others] = view.needs();
            let me = PathId(ident.0);
            put("own_key_is_none", others.get(&me).is_none() as u64);
            for (id, value) in acks.0.drain(..) {
                if id == me {
                    own.0 = value;
                    put("ack_own", value);
                } else if let Some(s) = others.get_mut(&id) {
                    s.0 = value;
                    put("ack_other", value);
                } else {
                    put("ack_unknown", id.0.into());
                }
            }
            put("batch", batches);
        });
    }
}

/// What the sweep hands the driver: a key it did not find.
#[derive(Debug, PartialEq, Eq)]
pub struct Missing(pub &'static str, pub u32);

/// On the connection: every path and every peer by key; a key not there
/// is yielded to the driver, one a poll.
pub struct Sweep;
impl<Q: Composition> CallKind<Q> for Sweep {
    type Needs = HList![
        At<Up<0>, Here, Mut<Probe>>,
        At<Up<0>, ToChild<Path, ByKey<PathId>>, MutList<HList![PathState, PathStreams]>>,
        At<Up<0>, ToChild<Peer, ByKey<PeerId>>, Mut<Budget>>,
    ];
    type Op = Sweeping;
    type Yield = Missing;
    fn start() -> Sweeping {
        Sweeping::new()
    }
}
#[call(yields = Missing)]
pub async fn sweeping<W: CallFamily<C = Sweep>>(host: &mut impl LentAccess<Family = W>) {
    loop {
        let missing = host.with(|mut view| {
            let hlist_pat![probe, paths, peers] = view.needs();
            let mut missing = Vec::new();
            for (id, v) in probe.paths.drain(..) {
                match paths.get_mut(&id) {
                    Some(hlist_pat![state, streams]) => {
                        state.0 = v;
                        streams.0 += 1;
                    }
                    None => missing.push(Missing("path", id.0)),
                }
            }
            for (id, v) in probe.peers.drain(..) {
                match peers.get_mut(&id) {
                    Some(b) => b.0 = v,
                    None => missing.push(Missing("peer", id.0)),
                }
            }
            missing
        });
        for m in missing {
            yield m;
        }
        next_poll().await;
    }
}

/// On a route: the linked peer, shared. The kind is the operation struct
/// `#[call]` generates.
impl<Q: Composition> CallKind<Q> for RouteTicking {
    type Needs = HList![
        At<Up<0>, Here, Mut<RouteState>>,
        At<Kind<Connection>, ToChild<Peer, MyLink>, Ref<Budget>>,
        At<Up<0>, ToChild<Timer>, Ref<Rtt>>,
    ];
    type Op = Self;
    fn start() -> Self {
        Self::new()
    }
}
#[call]
pub async fn route_ticking<W: CallFamily<C = RouteTicking>>(
    host: &mut impl LentAccess<Family = W>,
) {
    loop {
        host.with(|mut view| {
            let hlist_pat![state, budget, rtt] = view.needs();
            state.0 += budget.0 + rtt.map_or(0, |r| r.0);
            put("route", state.0);
        });
        next_poll().await;
    }
}

// ---------------------------------------------------------------- parts and compositions

macro_rules! part {
    ($part:ident { $($kind:ty => [$($comp:ty),*] calls [$($call:ty),*]),* $(,)? }) => {
        pub struct $part;
        $(
            impl Share<$kind> for $part { type Data = HList![$($comp),*]; }
            impl ShareCalls<$kind> for $part { type Calls = HList![$($call),*]; }
        )*
    };
}
part!(Base {
    Connection => [Keys, Clock, Probe] calls [Advance, Sweep],
    Session => [] calls [],
    Peer => [Budget] calls [],
    Path => [PathState, PathIdent, PathAcks] calls [PathAckReceive],
    Route => [RouteState] calls [],
    Timer => [Rtt] calls [],
});
part!(Streamed {
    Connection => [] calls [],
    Session => [] calls [],
    Peer => [] calls [],
    Path => [PathStreams] calls [],
    Route => [] calls [RouteTicking],
    Timer => [] calls [],
});
part!(Extended {
    Connection => [Extra] calls [],
    Session => [] calls [],
    Peer => [] calls [],
    Path => [] calls [],
    Route => [ExtraRoute] calls [],
    Timer => [] calls [],
});

pub struct Q2;
pub struct Q3;
composition!(impl for Q2: Connection, HList![Base, Streamed]);
composition!(impl for Q3: Connection, HList![Base, Streamed, Extended]);

fn component<X, I, D: Selector<X, I>>(d: &mut D) -> &mut X {
    d.get_mut()
}

macro_rules! build {
    ($q:ty) => {{
        let mut root: Node<Connection, <$q as Composition>::Parts> = new();
        component::<Keys, _, _>(&mut root.data.head).0 = 7;
        let peer: PeerAt = insert(&mut root, End, PeerId(7), new()).ok().unwrap();
        component::<Budget, _, _>(&mut peer.node_mut(&mut root).unwrap().data.head).0 = 100;
        let p0: PathAt = insert(&mut root, End, PathId(0), new()).ok().unwrap();
        let p2: PathAt = insert(&mut root, End, PathId(2), new()).ok().unwrap();
        component::<PathIdent, _, _>(&mut p2.node_mut(&mut root).unwrap().data.head).0 = 2;
        let route: RouteAt = insert(&mut root, p0, PeerId(7), new()).ok().unwrap();
        (root, peer, p0, p2, route)
    }};
}

fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

#[test]
fn a_call_without_keyed_needs_runs_to_its_end() {
    let (mut root, ..) = build!(Q2);
    let mut cx = cx();
    let mut call = pin!(start::<Q2, Advance, _, _>(&End));
    for _ in 0..2 {
        assert_eq!(
            poll::<Q2, Advance, _, _>(
                &mut root,
                End,
                &nitori_driver::event::Outbox::new(),
                call.as_mut(),
                &mut cx
            ),
            Ok(CallStep::Pending)
        );
    }
    assert_eq!(
        poll::<Q2, Advance, _, _>(
            &mut root,
            End,
            &nitori_driver::event::Outbox::new(),
            call.as_mut(),
            &mut cx
        ),
        Ok(CallStep::Finished)
    );
    assert_eq!(
        seen(),
        [("advance", 1007), ("advance", 2007), ("advance", 3007)]
    );
}

#[test]
fn a_call_waits_across_polls_and_sees_a_new_view_each_time() {
    let (mut root, _, p0, p2, _) = build!(Q2);
    let mut cx = cx();
    let mut call = pin!(start::<Q2, PathAckReceive, _, _>(&p0));
    for _ in 0..2 {
        assert_eq!(
            poll::<Q2, PathAckReceive, _, _>(
                &mut root,
                p0,
                &nitori_driver::event::Outbox::new(),
                call.as_mut(),
                &mut cx
            ),
            Ok(CallStep::Pending)
        );
    }
    assert_eq!(seen(), []);
    component::<PathAcks, _, _>(&mut p0.node_mut(&mut root).unwrap().data.head).0 =
        vec![(PathId(2), 41), (PathId(0), 40), (PathId(5), 1)];
    assert_eq!(
        poll::<Q2, PathAckReceive, _, _>(
            &mut root,
            p0,
            &nitori_driver::event::Outbox::new(),
            call.as_mut(),
            &mut cx
        ),
        Ok(CallStep::Pending)
    );
    assert_eq!(
        seen(),
        [
            ("own_key_is_none", 1),
            ("ack_other", 41),
            ("ack_own", 40),
            ("ack_unknown", 5),
            ("batch", 1)
        ]
    );
    assert_eq!(p2.node(&root).unwrap().data.head.head, PathState(41));
    // Between polls the tree changes; the next poll is lent the new one.
    component::<PathAcks, _, _>(&mut p0.node_mut(&mut root).unwrap().data.head).0 =
        vec![(PathId(2), 50)];
    assert_eq!(
        poll::<Q2, PathAckReceive, _, _>(
            &mut root,
            p0,
            &nitori_driver::event::Outbox::new(),
            call.as_mut(),
            &mut cx
        ),
        Ok(CallStep::Pending)
    );
    assert_eq!(
        seen(),
        [("own_key_is_none", 1), ("ack_other", 50), ("batch", 2)]
    );

    // The same call's source under another composition.
    let (mut root, _, p0, p2, _) = build!(Q3);
    let mut call = pin!(start::<Q3, PathAckReceive, _, _>(&p2));
    component::<PathAcks, _, _>(&mut p2.node_mut(&mut root).unwrap().data.head).0 =
        vec![(PathId(0), 9)];
    assert_eq!(
        poll::<Q3, PathAckReceive, _, _>(
            &mut root,
            p2,
            &nitori_driver::event::Outbox::new(),
            call.as_mut(),
            &mut cx
        ),
        Ok(CallStep::Pending)
    );
    assert_eq!(p0.node(&root).unwrap().data.head.head, PathState(9));
    seen();
}

#[test]
fn a_call_yields_to_the_driver() {
    let (mut root, peer, p0, ..) = build!(Q3);
    let mut cx = cx();
    let mut call = pin!(start::<Q3, Sweep, _, _>(&End));
    root.data.head.tail.tail.head.paths = vec![(PathId(0), 3), (PathId(9), 1)];
    root.data.head.tail.tail.head.peers = vec![(PeerId(7), 5), (PeerId(1), 1)];
    assert_eq!(
        poll::<Q3, Sweep, _, _>(
            &mut root,
            End,
            &nitori_driver::event::Outbox::new(),
            call.as_mut(),
            &mut cx
        ),
        Ok(CallStep::Yielded(Missing("path", 9)))
    );
    assert_eq!(
        poll::<Q3, Sweep, _, _>(
            &mut root,
            End,
            &nitori_driver::event::Outbox::new(),
            call.as_mut(),
            &mut cx
        ),
        Ok(CallStep::Yielded(Missing("peer", 1)))
    );
    assert_eq!(
        poll::<Q3, Sweep, _, _>(
            &mut root,
            End,
            &nitori_driver::event::Outbox::new(),
            call.as_mut(),
            &mut cx
        ),
        Ok(CallStep::Pending)
    );
    let path = &p0.node(&root).unwrap().data;
    assert_eq!(
        (&path.head.head, &path.tail.head.head),
        (&PathState(3), &PathStreams(1))
    );
    assert_eq!(peer.node(&root).unwrap().data.head.head, Budget(5));
}

#[test]
fn a_kind_that_is_its_operation_and_a_linked_node() {
    let (mut root, _, _, _, route) = build!(Q2);
    let mut cx = cx();
    let mut call = pin!(start::<Q2, RouteTicking, _, _>(&route));
    for _ in 0..2 {
        assert_eq!(
            poll::<Q2, RouteTicking, _, _>(
                &mut root,
                route,
                &nitori_driver::event::Outbox::new(),
                call.as_mut(),
                &mut cx
            ),
            Ok(CallStep::Pending)
        );
    }
    assert_eq!(seen(), [("route", 100), ("route", 200)]);
}

#[test]
fn a_node_gone_is_an_error() {
    let (mut root, _, p0, ..) = build!(Q2);
    let mut cx = cx();
    let mut call = pin!(start::<Q2, PathAckReceive, _, _>(&p0));
    nitori_driver::structure::remove(&mut root, p0).unwrap();
    assert_eq!(
        poll::<Q2, PathAckReceive, _, _>(
            &mut root,
            p0,
            &nitori_driver::event::Outbox::new(),
            call.as_mut(),
            &mut cx
        ),
        Err(nitori_driver::lend::LendError::NoNode)
    );
}

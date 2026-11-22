//! Lending: the anchors, the steps, exclusive and shared needs, children
//! by key, the linked node.
mod common;

use common::{http::*, net::*, new};
use frunk_core::{HList, hlist::Selector, hlist_pat};
use nitori_driver::{
    lend::{LendError, lend},
    need::{
        At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, MyLink, Provides, Ref, Then, ToChild, Up,
    },
    structure::{End, Node, NodePath, insert, remove},
};

fn component<X, I, D: Selector<X, I>>(d: &mut D) -> &mut X {
    d.get_mut()
}

/// A connection with the peers 7 and 8, the paths 0 and 2 (each knowing
/// its ID), the route 7 under the path 0 with its timer, and the session.
macro_rules! build {
    ($p:ty) => {{
        let mut root: Node<Connection, $p> = new();
        component::<Keys, _, _>(&mut root.data.head).0 = 0xaa;
        let _: SessionAt = insert(&mut root, End, (), new()).ok().unwrap();
        let peer: PeerAt = insert(&mut root, End, PeerId(7), new()).ok().unwrap();
        component::<Budget, _, _>(&mut peer.node_mut(&mut root).unwrap().data.head).0 = 100;
        let _: PeerAt = insert(&mut root, End, PeerId(8), new()).ok().unwrap();
        let p0: PathAt = insert(&mut root, End, PathId(0), new()).ok().unwrap();
        let p2: PathAt = insert(&mut root, End, PathId(2), new()).ok().unwrap();
        component::<PathIdent, _, _>(&mut p2.node_mut(&mut root).unwrap().data.head).0 = 2;
        let route: RouteAt = insert(&mut root, p0, PeerId(7), new()).ok().unwrap();
        let timer: TimerAt = insert(&mut root, route, (), new()).ok().unwrap();
        component::<Rtt, _, _>(&mut timer.node_mut(&mut root).unwrap().data.head).0 = 33;
        (root, peer, p0, p2, route)
    }};
}

/// The three anchors, an at-most-one child two levels down, the linked
/// peer, a component shared twice.
type RouteNeeds = HList![
    At<Kind<Connection>, Here, Ref<Keys>>,
    At<Up<1>, Here, Mut<PathState>>,
    At<Provides<RouteState>, Here, Mut<RouteState>>,
    At<Up<0>, ToChild<Timer>, Ref<Rtt>>,
    At<Kind<Connection>, ToChild<Peer, MyLink>, Mut<Budget>>,
    At<Kind<Connection>, ToChild<Session>, Mut<SessionRecv>>,
    At<Up<2>, Here, Ref<Keys>>,
];

#[test]
fn anchors_and_steps() {
    let (mut root, peer, p0, _, route) = build!(P2);
    let seen = lend::<RouteNeeds, _, _, _, _, _>(&mut root, route, |view| {
        let hlist_pat![keys, path, state, rtt, budget, session, keys_again] = view;
        path.0 += 1;
        state.0 = keys.0 + keys_again.0;
        budget.0 -= 1;
        if let Some(s) = session {
            s.0 = 5;
        }
        rtt.map(|r| r.0)
    })
    .unwrap();
    assert_eq!(seen, Some(33));
    assert_eq!(route.node(&root).unwrap().data.head.head, RouteState(0x154));
    assert_eq!(p0.node(&root).unwrap().data.head.head, PathState(1));
    assert_eq!(peer.node(&root).unwrap().data.head.head, Budget(99));

    // Without the session and the timer the at-most-one needs are `None`;
    // the same needs under four parts.
    let (mut root, _, _, _, route) = build!(P4);
    let session: SessionAt = nitori_driver::structure::Step::new((), End);
    remove(&mut root, session).unwrap();
    let timer: TimerAt = nitori_driver::structure::Append::append(route, ());
    remove(&mut root, timer).unwrap();
    lend::<RouteNeeds, _, _, _, _, _>(&mut root, route, |view| {
        let hlist_pat![_, _, _, rtt, _, session, _] = view;
        assert!(rtt.is_none() && session.is_none());
    })
    .unwrap();
}

#[test]
fn a_node_gone_is_an_error() {
    let (mut root, _, p0, _, route) = build!(P2);
    remove(&mut root, p0).unwrap();
    assert_eq!(
        lend::<RouteNeeds, _, _, _, _, _>(&mut root, route, |_| ()),
        Err(LendError::NoNode)
    );
}

#[test]
#[should_panic(expected = "exists only while that node exists")]
fn a_linked_node_gone_breaks_the_invariant() {
    let (mut root, peer, _, _, route) = build!(P2);
    remove(&mut root, peer).unwrap();
    let _ = lend::<RouteNeeds, _, _, _, _, _>(&mut root, route, |_| ());
}

/// On the connection: every path and every peer by key ("all"), one by
/// one and several at once; a key repeated finds `None` the second time.
#[test]
fn every_child_by_key() {
    let (mut root, _, p0, p2, _) = build!(P2);
    type Needs = HList![
        At<Up<0>, ToChild<Path, ByKey<PathId>>, Mut<PathState>>,
        At<Up<0>, ToChild<Peer, ByKey<PeerId>>, Ref<Budget>>,
    ];
    lend::<Needs, _, _, _, _, _>(&mut root, End, |view| {
        let hlist_pat![mut paths, peers] = view;
        assert_eq!(peers.get(&PeerId(7)), Some(&Budget(100)));
        assert_eq!(peers.get(&PeerId(9)), None);
        assert_eq!(peers.iter().count(), 2);
        paths.get_mut(&PathId(0)).unwrap().0 = 10;
        paths.get_mut(&PathId(2)).unwrap().0 = 20;
        if let [Some(a), Some(b), dup] = paths.get_many_mut([&PathId(0), &PathId(2), &PathId(0)]) {
            core::mem::swap(a, b);
            assert!(dup.is_none());
        } else {
            panic!("both paths are there");
        }
        for (_, s) in paths.iter_mut() {
            s.0 += 100;
        }
        assert!(paths.get(&PathId(5)).is_none());
    })
    .unwrap();
    assert_eq!(p0.node(&root).unwrap().data.head.head, PathState(120));
    assert_eq!(p2.node(&root).unwrap().data.head.head, PathState(110));
}

/// On a path: the other paths by key ("all but the one on the way"); its
/// own key finds `None`, while its own state is lent through `Up<0>`.
#[test]
fn every_other_child_by_key() {
    let (mut root, _, p0, p2, route) = build!(P4);
    type Needs = HList![
        At<Up<0>, Here, Mut<PathState>>,
        At<Up<0>, Here, Ref<PathIdent>>,
        At<Kind<Connection>, ToChild<Path, ByKeyOthers<PathId>>, Mut<PathState>>,
    ];
    lend::<Needs, _, _, _, _, _>(&mut root, p2, |view| {
        let hlist_pat![own, ident, mut others] = view;
        assert!(others.get(&PathId(ident.0)).is_none());
        own.0 = 2;
        others.get_mut(&PathId(0)).unwrap().0 = 1;
        assert_eq!(others.iter().count(), 1);
    })
    .unwrap();
    assert_eq!(p0.node(&root).unwrap().data.head.head, PathState(1));
    assert_eq!(p2.node(&root).unwrap().data.head.head, PathState(2));
    // From a route, two levels up, shared.
    type FromRoute = HList![
        At<Up<1>, Here, Mut<PathState>>,
        At<Kind<Connection>, ToChild<Path, ByKeyOthers<PathId>>, Ref<PathState>>,
    ];
    lend::<FromRoute, _, _, _, _, _>(&mut root, route, |view| {
        let hlist_pat![own, others] = view;
        assert_eq!(own.0, 1);
        assert_eq!(others.get(&PathId(2)), Some(&PathState(2)));
        assert!(others.get(&PathId(0)).is_none());
    })
    .unwrap();
}

/// Several components of every child by key, from the shares of two
/// parts; each child gives them in the order of the need.
#[test]
fn several_components_by_key() {
    let (mut root, _, p0, p2, _) = build!(P2);
    type Needs = HList![
        At<Up<0>, ToChild<Path, ByKey<PathId>>, MutList<HList![PathStreams, PathState]>>,
        At<Up<0>, Here, Mut<Clock>>,
    ];
    lend::<Needs, _, _, _, _, _>(&mut root, End, |view| {
        let hlist_pat![mut paths, clock] = view;
        clock.0 += 1;
        let hlist_pat![streams, state] = paths.get_mut(&PathId(0)).unwrap();
        streams.0 = 12;
        state.0 = 9;
        for (id, hlist_pat![_, state]) in paths.iter_mut() {
            state.0 += u64::from(id.0);
        }
        if let [Some(a), Some(b)] = paths.get_many_mut([&PathId(0), &PathId(2)]) {
            let hlist_pat![a_streams, _] = a;
            let hlist_pat![b_streams, _] = b;
            core::mem::swap(a_streams, b_streams);
        }
        let hlist_pat![streams, _] = paths.get(&PathId(2)).unwrap();
        assert_eq!(*streams, PathStreams(12));
    })
    .unwrap();
    assert_eq!(p0.node(&root).unwrap().data.head.head, PathState(9));
    assert_eq!(p2.node(&root).unwrap().data.head.head, PathState(2));
    // On a path, the other paths, several components.
    type Others = HList![
        At<
            Kind<Connection>,
            ToChild<Path, ByKeyOthers<PathId>>,
            MutList<HList![PathState, PathStreams]>,
        >,
    ];
    lend::<Others, _, _, _, _, _>(&mut root, p0, |view| {
        let hlist_pat![mut others] = view;
        assert!(others.get_mut(&PathId(0)).is_none());
        let hlist_pat![state, streams] = others.get_mut(&PathId(2)).unwrap();
        state.0 += 1;
        streams.0 = 0;
    })
    .unwrap();
    assert_eq!(p2.node(&root).unwrap().data.head.head, PathState(3));
}

/// A second structure: a stream's body through an at-most-one child, the
/// streams by key from the connection, steps chained with `Then`.
#[test]
fn a_second_structure() {
    let mut root: Node<HttpConnection, H1> = new();
    let _: ControlAt = insert(&mut root, End, (), new()).ok().unwrap();
    let s4: StreamAt = insert(&mut root, End, StreamId(4), new()).ok().unwrap();
    let _: StreamAt = insert(&mut root, End, StreamId(8), new()).ok().unwrap();
    let _: BodyAt = insert(&mut root, s4, (), new()).ok().unwrap();
    type OnStream = HList![
        At<Up<0>, Here, Mut<Headers>>,
        At<Up<0>, ToChild<Body>, Mut<Received>>,
        At<Kind<HttpConnection>, ToChild<Control>, Ref<Settings>>,
        At<Kind<HttpConnection>, Here, Ref<Settings>>,
        At<Kind<HttpConnection>, ToChild<Stream, ByKeyOthers<StreamId>>, Ref<Headers>>,
    ];
    lend::<OnStream, _, _, _, _, _>(&mut root, s4, |view| {
        let hlist_pat![headers, body, control, settings, others] = view;
        headers.0.push("a");
        body.unwrap().0 += settings.0 + 1;
        assert!(control.is_some());
        assert!(others.get(&StreamId(4)).is_none());
        assert!(others.get(&StreamId(8)).is_some());
    })
    .unwrap();
    type OnConnection = HList![
        At<Up<0>, ToChild<Stream, ByKey<StreamId>>, Mut<Headers>>,
        At<Up<0>, Here, Mut<Limits>>,
        At<Up<0>, Then<ToChild<Control>, Here>, Mut<Settings>>,
    ];
    lend::<OnConnection, _, _, _, _, _>(&mut root, End, |view| {
        let hlist_pat![mut streams, limits, control] = view;
        limits.0 = streams.iter().count() as u64;
        assert_eq!(streams.get_mut(&StreamId(4)).unwrap().0, ["a"]);
        control.unwrap().0 = 3;
    })
    .unwrap();
}

//! The structure: adding, finding, removing and moving nodes, typed paths
//! and addresses, the order of removal.
mod common;

use core::any::type_name;

use common::{http::*, net::*, new};
use frunk_core::hlist::Selector;
use nitori_driver::structure::{
    Address, Append, End, InsertError, MoveError, Node, NodePath, PathFrom, Visit, addresses_under,
    insert, move_to, removal_order, remove, walk_node,
};

fn component<X, I, D: Selector<X, I>>(d: &mut D) -> &mut X {
    d.get_mut()
}

/// A connection with the peers 7 and 8, the paths 0 and 2, the route 7
/// under the path 0 with its timer, and the session.
macro_rules! build {
    ($p:ty) => {{
        let mut root: Node<Connection, $p> = new();
        let _: SessionAt = insert(&mut root, End, (), new()).ok().unwrap();
        let peer: PeerAt = insert(&mut root, End, PeerId(7), new()).ok().unwrap();
        let _: PeerAt = insert(&mut root, End, PeerId(8), new()).ok().unwrap();
        let p0: PathAt = insert(&mut root, End, PathId(0), new()).ok().unwrap();
        let p2: PathAt = insert(&mut root, End, PathId(2), new()).ok().unwrap();
        let route: RouteAt = insert(&mut root, p0, PeerId(7), new()).ok().unwrap();
        let timer: TimerAt = insert(&mut root, route, (), new()).ok().unwrap();
        (root, peer, p0, p2, route, timer)
    }};
}

#[test]
fn insert_find_and_remove() {
    let (mut root, peer, p0, p2, route, timer) = build!(P2);
    // The child's position is inferred from its kind; its data is reached
    // through the path.
    component::<Rtt, _, _>(&mut timer.node_mut(&mut root).unwrap().data.head).0 = 33;
    component::<Budget, _, _>(&mut peer.node_mut(&mut root).unwrap().data.head).0 = 5;
    assert_eq!(timer.node(&root).unwrap().data.head.head, Rtt(33));
    // A key already there: the node comes back.
    match insert(&mut root, End, PathId(2), new::<Path, P2>()) {
        Err(InsertError::Occupied(_)) => {}
        _ => panic!("the path 2 is there"),
    }
    // A parent not there.
    let gone: PathAt = insert(&mut root, End, PathId(9), new()).ok().unwrap();
    remove(&mut root, gone).unwrap();
    assert!(matches!(
        insert(&mut root, gone, PeerId(1), new::<Route, P2>()),
        Err(InsertError::NoParent(_))
    ));
    // Removing the path 0 takes its route and the route's timer with it.
    let removed = remove(&mut root, p0).unwrap();
    assert!(route.node(&root).is_none());
    assert!(timer.node(&root).is_none());
    drop(removed);
    assert!(p2.node(&root).is_some());
}

#[test]
fn move_keeps_the_node_and_its_key() {
    let (mut root, _, p0, p2, route, timer) = build!(P4);
    component::<Rtt, _, _>(&mut timer.node_mut(&mut root).unwrap().data.head).0 = 41;
    let moved: RouteAt = move_to(&mut root, route, p2).unwrap();
    assert!(route.node(&root).is_none());
    // The timer moved with the route.
    let moved_timer: TimerAt = moved.append(());
    assert_eq!(moved_timer.node(&root).unwrap().data.head.head, Rtt(41));
    // The destination has the key already: the route goes back.
    let back: RouteAt = insert(&mut root, p0, PeerId(7), new()).ok().unwrap();
    assert_eq!(move_to(&mut root, back, p2), Err(MoveError::Occupied));
    assert!(back.node(&root).is_some());
    // No destination.
    let gone: PathAt = insert(&mut root, End, PathId(9), new()).ok().unwrap();
    remove(&mut root, gone).unwrap();
    assert_eq!(
        move_to(&mut root, back, gone),
        Err(MoveError::NoDestination)
    );
    assert!(back.node(&root).is_some());
}

/// The address follows from the kinds alone: the same type for two lists
/// of parts, no part in its name, equal for the same handles.
#[test]
fn addresses_name_no_part() {
    let (root2, _, _, _, route2, _) = build!(P2);
    let (root4, _, _, _, route4, _) = build!(P4);
    let a2: Address<Connection> = route2.address();
    let a4: Address<Connection> = route4.address();
    assert_eq!(a2, a4);
    let name = type_name::<Address<Connection>>();
    assert!(
        !name.contains("Core") && !name.contains("Streaming"),
        "{name}"
    );
    let all2 = addresses_under(&root2);
    let all4 = addresses_under(&root4);
    assert_eq!(all2, all4);
    assert_eq!(all2.len(), 8);
    assert_eq!(all2[0], Address::here());
    assert!(all2.contains(&a2));
}

struct CountTimers(u32);
impl<P> Visit<P> for CountTimers {
    fn node<
        K: nitori_driver::structure::KindFor<P, Children: nitori_driver::structure::ChildAddresses<P>>,
    >(
        &mut self,
        _: &mut Node<K, P>,
    ) {
        if core::any::TypeId::of::<K>() == core::any::TypeId::of::<Timer>() {
            self.0 += 1;
        }
    }
}

#[test]
fn walk_an_address() {
    let (mut root, _, p0, _, _, timer) = build!(P2);
    let mut v = CountTimers(0);
    assert!(walk_node(&mut root, &timer.address(), &mut v));
    assert!(walk_node(&mut root, &p0.address(), &mut v));
    assert_eq!(v.0, 1);
    remove(&mut root, timer).unwrap();
    assert!(!walk_node(&mut root, &timer.address(), &mut v));
}

/// Children before their parent; a peer, which routes link to, after the
/// routes; the root last.
#[test]
fn removal_order_puts_linked_nodes_after_their_links() {
    let (root, peer, p0, p2, route, timer) = build!(P2);
    let order = removal_order(&root);
    let at = |a: Address<Connection>| order.iter().position(|x| *x == a).unwrap();
    assert_eq!(order.len(), 8);
    assert!(at(timer.address()) < at(route.address()));
    assert!(at(route.address()) < at(p0.address()));
    assert!(at(route.address()) < at(peer.address()));
    assert!(at(p2.address()) < at(peer.address()));
    assert_eq!(*order.last().unwrap(), Address::here());
}

#[test]
fn a_second_structure() {
    let mut root: Node<HttpConnection, H1> = new();
    let _: ControlAt = insert(&mut root, End, (), new()).ok().unwrap();
    let s4: StreamAt = insert(&mut root, End, StreamId(4), new()).ok().unwrap();
    let s8: StreamAt = insert(&mut root, End, StreamId(8), new()).ok().unwrap();
    let body: BodyAt = insert(&mut root, s4, (), new()).ok().unwrap();
    body.node_mut(&mut root).unwrap().data.head.head.0 = 10;
    assert_eq!(addresses_under(&root).len(), 5);
    remove(&mut root, s4).unwrap();
    assert!(body.node(&root).is_none());
    assert!(s8.node(&root).is_some());
    let order = removal_order(&root);
    assert_eq!(order.len(), 3);
    let name = type_name::<Address<HttpConnection>>();
    assert!(
        !name.contains("Http,") && !name.contains("::Http>"),
        "{name}"
    );
}

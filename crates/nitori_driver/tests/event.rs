//! Publishing through needs: a publisher at an anchor on the way up puts
//! the event in the outbox with the depth of its node.
mod common;

use common::{net::*, new};
use frunk_core::{HList, hlist_pat};
use nitori_driver::{
    event::{Event, Outbox, Publish},
    lend::lend,
    need::{At, Here, Kind, Mut, Up},
    structure::{End, insert},
};

pub struct Arrived;
impl Event for Arrived {}
pub struct Changed;
impl Event for Changed {}

#[test]
fn publish_at_the_node_and_above() {
    let mut root = new::<Connection, P2>();
    let path: PathAt = insert(&mut root, End, PathId(1), new()).ok().unwrap();
    type Needs = HList![
        At<Up<0>, Here, Mut<PathState>>,
        At<Up<0>, Here, Publish<Arrived>>,
        At<Kind<Connection>, Here, Publish<Changed>>,
    ];
    let outbox = Outbox::new();
    lend::<Needs, _, _, _, _, _>(&mut root, path, &outbox, |view| {
        let hlist_pat![state, arrived, changed] = view;
        state.0 += 1;
        arrived.publish();
        changed.publish();
        arrived.publish();
    })
    .unwrap();
    let id = core::any::TypeId::of::<Arrived>;
    let ch = core::any::TypeId::of::<Changed>;
    assert_eq!(outbox.take(), [(1, id()), (0, ch()), (1, id())]);
    assert!(outbox.take().is_empty());
}

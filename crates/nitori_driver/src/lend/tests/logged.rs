//! A generic component on several kinds: each kind's is another
//! component.
use core::marker::PhantomData;

use alloc::{string::String, vec::Vec};
use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{LinkId, lend};
use crate::{
    component::Part,
    id::Numbered,
    need::{Mut, Under},
    structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, insert},
};

#[derive(Numbered)]
struct Logged<K> {
    lines: Vec<String>,
    _kind: PhantomData<fn() -> K>,
}
impl<K> Logged<K> {
    fn new(first: &str) -> Self {
        Self {
            lines: alloc::vec![first.into()],
            _kind: PhantomData,
        }
    }
}

/// `Conn { Logged<Conn>; [Link by LinkId { Logged<Link>; Rec? { Logged<Rec>, Logged<Link> } }] }`:
/// the recovery has the link's log too, so from it two nodes on the way up
/// have `Logged<Link>`.
#[derive(Numbered)]
struct Rec;
impl NodeKind for Rec {
    type Children = HList![];
}
#[derive(Numbered)]
struct Link;
impl NodeKind for Link {
    type Children = HList![Child<Rec, AtMostOne>];
}
#[derive(Numbered)]
struct Conn;
impl NodeKind for Conn {
    type Children = HList![Child<Link, SlotVec<LinkId>>];
}
struct Logs;
impl<L> Part<L> for Logs {
    type Args = ();
    type Shares = HList![
        Share<Conn, HList![Logged<Conn>]>,
        Share<Link, HList![Logged<Link>]>,
        Share<Rec, HList![Logged<Rec>, Logged<Link>]>,
    ];
}
type Parts = HList![Logs];

fn tree() -> Node<Conn, Parts> {
    let mut root = Node::new(hlist![hlist![Logged::<Conn>::new("conn")]]);
    for l in [1, 2] {
        let link = insert(
            &mut root,
            End,
            LinkId(l),
            Node::new(hlist![hlist![Logged::<Link>::new("link")]]),
        )
        .ok()
        .expect("the parent is there and the key is free");
        if l == 1 {
            insert(
                &mut root,
                link,
                (),
                Node::new(hlist![hlist![
                    Logged::<Rec>::new("rec"),
                    Logged::<Link>::new("rec as link")
                ]]),
            )
            .ok()
            .expect("the parent is there and the key is free");
        }
    }
    root
}

#[test]
fn each_kinds_log_is_its_own_component() {
    let mut root = tree();
    let got = lend!(
        &mut root,
        Rec,
        HList![Mut<Logged<Rec>>, Mut<Logged<Conn>>],
        hlist![LinkId(1)],
        |hlist_pat![rec, conn]| {
            rec.lines.push("r".into());
            conn.lines.push("c".into());
            (rec.lines.clone(), conn.lines.clone())
        }
    );
    assert_eq!(
        got,
        Some((
            alloc::vec!["rec".into(), "r".into()],
            alloc::vec!["conn".into(), "c".into()]
        ))
    );
    let got = lend!(
        &mut root,
        Link,
        HList![Mut<Logged<Link>>],
        hlist![LinkId(2)],
        |hlist_pat![l]| l.lines.clone()
    );
    assert_eq!(got, Some(alloc::vec![String::from("link")]));
    let got = lend!(
        &mut root,
        Conn,
        HList![Under<Logged<Conn>, Mut<Logged<Rec>>>],
        hlist![],
        |hlist_pat![v]| { v.keys().copied().collect::<Vec<_>>() }
    );
    assert_eq!(got, Some(alloc::vec![LinkId(1)]));
}

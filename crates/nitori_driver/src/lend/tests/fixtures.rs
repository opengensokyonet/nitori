//! The components, kinds and parts of the two structures, and their trees.
//!
//! Structure one: `Conn1 { ConnCore, Keys, RecoveryFactory; [Entry by
//! LinkId], [LogicalPath by PathKey { SentRecords; [LinkPath by LinkId {
//! LinkCore, Planner; Recovery? { Congestion } }] }] }`. Structure two:
//! `Conn2 { ConnCore, Keys, SentRecords, RecoveryFactory; [Entry by
//! LinkId], [LinkPath by LinkId] }`. The components are split between two
//! parts, `Core` and `Loss`.
use frunk_core::{HList, hlist};

use crate::{
    id::Numbered,
    structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, insert},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathKey(pub u32);

#[derive(Debug, Default, Numbered)]
pub struct ConnCore {
    pub rx: u32,
}
#[derive(Debug, Default, Numbered)]
pub struct Keys {
    pub epoch: u32,
}
#[derive(Debug, Default, Numbered)]
pub struct Credit {
    pub available: u32,
}
#[derive(Debug, Default, Numbered)]
pub struct LinkLimit {
    pub max: u32,
}
#[derive(Debug, Default, Numbered)]
pub struct SentRecords {
    pub sent: alloc::vec::Vec<u64>,
    pub acked: u32,
}
#[derive(Debug, Default, Numbered)]
pub struct LinkCore {
    pub rx: u32,
}
#[derive(Debug, Default, Numbered)]
pub struct Planner {
    pub next_pn: u64,
}
#[derive(Debug, Default, Numbered)]
pub struct RecoveryFactory {
    pub made: u32,
}
#[derive(Debug, Default, Numbered)]
pub struct Congestion {
    pub in_flight: u32,
}

pub struct Entry;
impl NodeKind for Entry {
    type Children = HList![];
}
pub struct Recovery;
impl NodeKind for Recovery {
    type Children = HList![];
}
pub struct LinkPath;
impl NodeKind for LinkPath {
    type Children = HList![Child<Recovery, AtMostOne>];
}
pub struct LogicalPath;
impl NodeKind for LogicalPath {
    type Children = HList![Child<LinkPath, SlotVec<LinkId>>];
}
pub struct Conn1;
impl NodeKind for Conn1 {
    type Children = HList![
        Child<Entry, SlotVec<LinkId>>,
        Child<LogicalPath, SlotVec<PathKey>>
    ];
}
pub struct Conn2;
impl NodeKind for Conn2 {
    type Children = HList![Child<Entry, SlotVec<LinkId>>, Child<LinkPath, SlotVec<LinkId>>];
}

/// The part with the connection's, the links' and the entries' state.
pub struct Core;
/// The part with what is sent and the recovery.
pub struct Loss;
pub type Parts = HList![Core, Loss];

impl Share<Conn1> for Core {
    type Data = HList![ConnCore, Keys];
}
impl Share<Conn1> for Loss {
    type Data = HList![RecoveryFactory];
}
impl Share<Conn2> for Core {
    type Data = HList![ConnCore, Keys];
}
impl Share<Conn2> for Loss {
    type Data = HList![SentRecords, RecoveryFactory];
}
impl Share<Entry> for Core {
    type Data = HList![Credit, LinkLimit];
}
impl Share<Entry> for Loss {
    type Data = HList![];
}
impl Share<LogicalPath> for Core {
    type Data = HList![];
}
impl Share<LogicalPath> for Loss {
    type Data = HList![SentRecords];
}
impl Share<LinkPath> for Core {
    type Data = HList![LinkCore, Planner];
}
impl Share<LinkPath> for Loss {
    type Data = HList![];
}
impl Share<Recovery> for Core {
    type Data = HList![];
}
impl Share<Recovery> for Loss {
    type Data = HList![Congestion];
}

fn entry(l: u32) -> Node<Entry, Parts> {
    Node::new(hlist![
        hlist![Credit { available: 10 * l }, LinkLimit { max: 100 * l }],
        hlist![]
    ])
}
fn link_path(l: u32) -> Node<LinkPath, Parts> {
    Node::new(hlist![
        hlist![
            LinkCore::default(),
            Planner {
                next_pn: 100 * u64::from(l)
            }
        ],
        hlist![]
    ])
}
fn recovery() -> Node<Recovery, Parts> {
    Node::new(hlist![hlist![], hlist![Congestion::default()]])
}

/// Structure one: entries 1–3; logical paths 10 (links 1 and 2) and 20
/// (link 3); recovery on (10, 1) and (20, 3).
pub fn tree_one() -> Node<Conn1, Parts> {
    let mut root = Node::new(hlist![
        hlist![ConnCore::default(), Keys { epoch: 7 }],
        hlist![RecoveryFactory { made: 5 }]
    ]);
    for l in [1, 2, 3] {
        insert(&mut root, End, LinkId(l), entry(l))
            .ok()
            .expect("the parent is there and the key is free");
    }
    for (p, links) in [(10, &[1, 2][..]), (20, &[3][..])] {
        let path = insert(
            &mut root,
            End,
            PathKey(p),
            Node::<LogicalPath, Parts>::new(hlist![hlist![], hlist![SentRecords::default()]]),
        )
        .ok()
        .expect("the parent is there and the key is free");
        for &l in links {
            let link = insert(&mut root, path, LinkId(l), link_path(l))
                .ok()
                .expect("the parent is there and the key is free");
            if l != 2 {
                insert(&mut root, link, (), recovery())
                    .ok()
                    .expect("the parent is there and the key is free");
            }
        }
    }
    root
}

/// Structure two: entries 1 and 2; link paths 1 and 2; recovery on 1.
pub fn tree_two() -> Node<Conn2, Parts> {
    let mut root = Node::new(hlist![
        hlist![ConnCore::default(), Keys { epoch: 3 }],
        hlist![SentRecords::default(), RecoveryFactory { made: 7 }]
    ]);
    for l in [1, 2] {
        insert(&mut root, End, LinkId(l), entry(l))
            .ok()
            .expect("the parent is there and the key is free");
        let link = insert(&mut root, End, LinkId(l), link_path(l))
            .ok()
            .expect("the parent is there and the key is free");
        if l == 1 {
            insert(&mut root, link, (), recovery())
                .ok()
                .expect("the parent is there and the key is free");
        }
    }
    root
}

/// Lend the needs `$needs` at the node of the kind `$kind` with the full
/// key `$at` of the tree `$root` to `$f`.
macro_rules! lend {
    ($root:expr, $kind:ty, $needs:ty, $at:expr, $f:expr) => {
        $crate::lend::lend::<_, _, $kind, _, $needs, _, _, _, _>($root, &$at, &mut (), $f)
    };
}
pub(crate) use lend;

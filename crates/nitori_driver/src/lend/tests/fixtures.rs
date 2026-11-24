//! The components, kinds and parts of the two structures, and their trees.
//!
//! Structure one: `Conn1 { ConnCore, Keys, RecoveryFactory; [LogicalPath by
//! PathKey { SentRecords; [Entry by LinkId], [LinkPath by LinkId {
//! LinkCore, Planner; Recovery? { Congestion } }] }] }`. Structure two:
//! `Conn2 { ConnCore, Keys, SentRecords, RecoveryFactory; [Entry by
//! LinkId], [LinkPath by LinkId] }`. The components are split between
//! three parts, `Core`, `Loss` and `Watch`; the last keeps only a held
//! estimator on the link paths. What the parts react to is in [`parts`].
use frunk_core::{HList, hlist};

use super::parts::*;
use crate::{
    component::Held,
    id::Numbered,
    react::{NodeEnding, NodeStarted, ReactsTo},
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

#[derive(Numbered)]
pub struct Entry;
impl NodeKind for Entry {
    type Children = HList![];
}
#[derive(Numbered)]
pub struct Recovery;
impl NodeKind for Recovery {
    type Children = HList![];
}
/// A link path starts from a number its planner counts from, and ends
/// for a code.
#[derive(Numbered)]
pub struct LinkPath;
impl NodeKind for LinkPath {
    type Children = HList![Child<Recovery, AtMostOne>];
    type Start = u64;
    type EndReason = u8;
}
#[derive(Numbered)]
pub struct LogicalPath;
impl NodeKind for LogicalPath {
    type Children = HList![Child<Entry, SlotVec<LinkId>>, Child<LinkPath, SlotVec<LinkId>>];
}
/// A connection can be asked to close, for a code.
#[derive(Numbered)]
pub struct Conn1;
impl NodeKind for Conn1 {
    type Children = HList![Child<LogicalPath, SlotVec<PathKey>>];
    type Request = CloseConn;
    type Handler = Closing;
}
#[derive(Numbered)]
pub struct Conn2;
impl NodeKind for Conn2 {
    type Children = HList![Child<Entry, SlotVec<LinkId>>, Child<LinkPath, SlotVec<LinkId>>];
    type Request = CloseConn;
    type Handler = Closing;
}
/// What a connection is asked: to close, for a code.
#[derive(Debug, PartialEq, Eq)]
pub struct CloseConn(pub u32);
/// A connection's handler: it ends the connection.
pub struct Closing;
impl<K: NodeKind<Request = CloseConn, EndReason = ()>, Pr> crate::request::HandleRequest<K, Pr>
    for Closing
{
    type Needs = HList![];
    fn handle<P: crate::driver::HandlerFor<K, Self, Pr>>(
        edit: crate::driver::Edit<'_, K, P>,
        _: CloseConn,
    ) {
        edit.end_self(());
    }
}

/// The part with the connection's, the links' and the entries' state.
pub struct Core;
/// The part with what is sent and the recovery.
pub struct Loss;
/// The part that keeps an estimator on the link paths.
pub struct Watch;
pub type Parts = HList![Core, Loss, Watch];

/// What the core reacts to at the connection, of either structure.
type CoreAtConnReacts = HList![
    ReactsTo<Probe, CoreAtConn>,
    ReactsTo<NodeStarted<LinkPath>, StartedAtConn>,
    ReactsTo<NodeEnding<LinkPath>, EndingAtConn>,
];
/// What the loss part reacts to at the connection, of either structure.
type LossAtConnReacts = HList![
    ReactsTo<PacketsSent, RA2>,
    ReactsTo<P2, RC2>,
    ReactsTo<Probe, LossAtConn>,
];

impl Share<Conn1> for Core {
    type Data = HList![ConnCore, Keys];
    type Reacts = CoreAtConnReacts;
}
impl Share<Conn1> for Loss {
    type Data = HList![RecoveryFactory];
    type Reacts = LossAtConnReacts;
}
impl Share<Conn2> for Core {
    type Data = HList![ConnCore, Keys];
    type Reacts = CoreAtConnReacts;
}
impl Share<Conn2> for Loss {
    type Data = HList![SentRecords, RecoveryFactory];
    type Reacts = LossAtConnReacts;
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
    type Calls = crate::call::tests::CoreLinkCalls;
    type Reacts = HList![
        ReactsTo<PacketsSent, RA1>,
        ReactsTo<P1, RC1>,
        ReactsTo<Probe, CoreAtLink>,
        ReactsTo<NodeStarted<LinkPath>, StartedAtLink>,
        ReactsTo<NodeEnding<LinkPath>, EndingAtLink>,
        ReactsTo<Pinged, PingIt>,
    ];
}
impl Share<LinkPath> for Loss {
    type Data = HList![];
    type Calls = crate::call::tests::LossLinkCalls;
    type Reacts = HList![ReactsTo<Probe, LossAtLink>, ReactsTo<Survey, SurveyAtLink>];
}
impl Share<Recovery> for Core {
    type Data = HList![];
}
impl Share<Recovery> for Loss {
    type Data = HList![Congestion];
}
impl Share<Conn1> for Watch {
    type Data = HList![];
}
impl Share<Conn2> for Watch {
    type Data = HList![];
}
impl Share<Entry> for Watch {
    type Data = HList![];
}
impl Share<LogicalPath> for Watch {
    type Data = HList![];
}
impl Share<LinkPath> for Watch {
    type Data = HList![Held<Rtt>];
    type Reacts = HList![ReactsTo<Unrelated, RZ>];
}
impl Share<Recovery> for Watch {
    type Data = HList![];
}

fn entry(l: u32) -> Node<Entry, Parts> {
    Node::new(hlist![
        hlist![Credit { available: 10 * l }, LinkLimit { max: 100 * l }],
        hlist![],
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
        hlist![],
        hlist![Held(Rtt::default())]
    ])
}
fn recovery() -> Node<Recovery, Parts> {
    Node::new(hlist![hlist![], hlist![Congestion::default()], hlist![]])
}

/// Structure one: logical paths 10 (entries and links 1 and 2) and 20
/// (entry and link 3); recovery on (10, 1) and (20, 3).
pub fn tree_one() -> Node<Conn1, Parts> {
    let mut root = Node::new(hlist![
        hlist![ConnCore::default(), Keys { epoch: 7 }],
        hlist![RecoveryFactory { made: 5 }],
        hlist![]
    ]);
    for (p, links) in [(10, &[1, 2][..]), (20, &[3][..])] {
        let path = insert(
            &mut root,
            End,
            PathKey(p),
            Node::<LogicalPath, Parts>::new(hlist![
                hlist![],
                hlist![SentRecords::default()],
                hlist![]
            ]),
        )
        .ok()
        .expect("the parent is there and the key is free");
        for &l in links {
            insert(&mut root, path, LinkId(l), entry(l))
                .ok()
                .expect("the parent is there and the key is free");
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

/// Structure one holding what structure two does: logical path 10 with
/// entries and links 1 and 2, recovery on 1, and the connection's state of
/// structure two.
pub fn tree_one_as_two() -> Node<Conn1, Parts> {
    let mut root = Node::new(hlist![
        hlist![ConnCore::default(), Keys { epoch: 3 }],
        hlist![RecoveryFactory { made: 7 }],
        hlist![]
    ]);
    let path = insert(
        &mut root,
        End,
        PathKey(10),
        Node::<LogicalPath, Parts>::new(hlist![hlist![], hlist![SentRecords::default()], hlist![]]),
    )
    .ok()
    .expect("the parent is there and the key is free");
    for l in [1, 2] {
        insert(&mut root, path, LinkId(l), entry(l))
            .ok()
            .expect("the parent is there and the key is free");
        let link = insert(&mut root, path, LinkId(l), link_path(l))
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

/// Structure two: entries 1 and 2; link paths 1 and 2; recovery on 1.
pub fn tree_two() -> Node<Conn2, Parts> {
    let mut root = Node::new(hlist![
        hlist![ConnCore::default(), Keys { epoch: 3 }],
        hlist![SentRecords::default(), RecoveryFactory { made: 7 }],
        hlist![]
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

/// [`lend!`], with the context `$cx`.
macro_rules! lend_in {
    ($root:expr, $kind:ty, $needs:ty, $at:expr, $cx:expr, $f:expr) => {
        $crate::lend::lend::<_, _, $kind, _, $needs, _, _, _, _>($root, &$at, $cx, $f)
    };
}
pub(crate) use lend_in;

/// A clock whose instants are durations since it started.
#[derive(Clone, Debug)]
pub struct TestClock {
    /// The last instant it can give.
    pub last: core::time::Duration,
}
impl nitori_clock::MonotonicClock for TestClock {
    type Instant = core::time::Duration;
    type Sleep = core::future::Ready<()>;
    fn now(&self) -> core::time::Duration {
        core::time::Duration::ZERO
    }
    fn checked_add(
        &self,
        at: core::time::Duration,
        by: core::time::Duration,
    ) -> Option<core::time::Duration> {
        at.checked_add(by).filter(|t| *t <= self.last)
    }
    fn sleep_until(&self, _: core::time::Duration) -> Self::Sleep {
        core::future::ready(())
    }
    fn timer_granularity(&self) -> core::time::Duration {
        core::time::Duration::from_millis(1)
    }
}

/// A request to probe a link.
#[derive(Debug, PartialEq, Eq)]
pub struct Ping(pub LinkId);
/// A request to close the connection.
#[derive(Debug, PartialEq, Eq)]
pub struct Close;

/// The context the tests lend with: a clock read at one second, and boxes
/// for pings and closes.
pub type TestCx = crate::lend::context::Context<
    HList![
        crate::need::LentClock<TestClock>,
        crate::lend::context::RequestList<Ping>,
        crate::lend::context::RequestList<Close>,
    ],
>;

/// The context, read at one second, with empty boxes.
pub fn test_cx() -> TestCx {
    crate::lend::context::Context::new(hlist![
        crate::need::LentClock::new(
            TestClock {
                last: core::time::Duration::from_secs(10)
            },
            core::time::Duration::from_secs(1)
        ),
        crate::lend::context::RequestList::new(),
        crate::lend::context::RequestList::new(),
    ])
}

/// The pings and the closes put in the boxes of `cx`, each with its place
/// in the order, leaving the boxes empty.
pub fn requests(cx: &mut TestCx) -> (alloc::vec::Vec<(u64, Ping)>, alloc::vec::Vec<(u64, Close)>) {
    let frunk_core::hlist_pat![_, pings, closes] = cx.pieces_mut();
    (pings.take(), closes.take())
}

/// The results of one test on structure one and on structure two are the
/// same, and are `want`.
pub fn same_on_both<O: PartialEq + core::fmt::Debug>(one: O, two: O, want: O) {
    assert_eq!(one, two, "structure one and structure two differ");
    assert_eq!(one, want);
}

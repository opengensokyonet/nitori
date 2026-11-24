//! What a lending walks: down the route to the lend point by key, and
//! below each anchor the nodes the needs take, each member given its own.
use alloc::{vec, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::{
    need::{At, ExactlyOne, Locator, Mut, OneAtATime, OwnKey, Pos, Ref, Under},
    needs,
};

#[test]
fn two_anchors_over_one_kind() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Under<SentRecords, Ref<Planner>>, Under<ConnCore, Ref<Planner>>],
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![path, conn]| {
            let path: Vec<_> = path.iter().map(|(k, p)| (*k, p.next_pn)).collect();
            let conn: Vec<_> = conn.iter().map(|(k, p)| (*k, p.next_pn)).collect();
            (path, conn)
        }
    );
    assert_eq!(
        got,
        Some((
            vec![(LinkId(1), 100), (LinkId(2), 200)],
            vec![
                ((PathKey(10), LinkId(1)), 100),
                ((PathKey(10), LinkId(2)), 200),
                ((PathKey(20), LinkId(3)), 300)
            ]
        ))
    );
}

#[test]
fn a_lend_point_missing_on_the_way_down() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Under<ConnCore, Ref<Planner>>],
        hlist![PathKey(30), LinkId(1)],
        |_| ()
    );
    assert_eq!(got, None);
}

/// By logical path, the link cores below it, and every link core of the
/// connection.
type ByPath = HList![Under<
    ConnCore,
    At<
        SentRecords,
        HList![
            Ref<SentRecords>,
            Under<SentRecords, Ref<LinkCore>>,
            Under<ConnCore, Ref<LinkCore>>
        ],
    >,
>];

#[test]
fn nested_unders_at_and_above_each_member() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, ByPath, hlist![], |hlist_pat![v]| {
        v.iter()
            .map(|(p, hlist_pat![_, own, all])| (*p, own.len(), all.len()))
            .collect::<Vec<_>>()
    });
    assert_eq!(got, Some(vec![(PathKey(10), 2, 3), (PathKey(20), 1, 3)]));
}

/// [`ByPath`], one logical path at a time.
type ByPathOne = HList![Under<
    ConnCore,
    OneAtATime<
        At<
            SentRecords,
            HList![
                Ref<SentRecords>,
                Under<SentRecords, Ref<LinkCore>>,
                Under<ConnCore, Ref<LinkCore>>
            ],
        >,
    >,
>];

#[test]
fn nested_unders_one_member_at_a_time() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, ByPathOne, hlist![], |hlist_pat![
        mut v
    ]| {
        let mut seen = Vec::new();
        v.for_each_mut(|p, hlist_pat![_, own, all]| {
            seen.push((*p, own.keys().copied().collect::<Vec<_>>(), all.len()));
        });
        seen
    });
    assert_eq!(
        got,
        Some(vec![
            (PathKey(10), vec![LinkId(1), LinkId(2)], 3),
            (PathKey(20), vec![LinkId(3)], 3)
        ])
    );
}

/// By logical path, every link core of the connection in a `+`: the
/// union no longer says that the `Under` is anchored above the member.
type PlusAbove = HList![Under<
    ConnCore,
    At<SentRecords, needs![(Mut<SentRecords> + Under<ConnCore, Ref<LinkCore>>)]>,
>];

#[test]
fn a_plus_at_each_member_reads_above_it() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, PlusAbove, hlist![], |hlist_pat![
        mut v
    ]| {
        let mut seen = Vec::new();
        for (k, hlist_pat![plus]) in v.iter_mut() {
            let all = plus.take::<Under<ConnCore, Ref<LinkCore>>, _>();
            seen.push((*k, all.len()));
        }
        seen
    });
    assert_eq!(got, Some(vec![(PathKey(10), 3), (PathKey(20), 3)]));
}

/// [`PlusAbove`], one logical path at a time.
type PlusAboveOne = HList![Under<
    ConnCore,
    OneAtATime<At<SentRecords, needs![(Mut<SentRecords> + Under<ConnCore, Ref<LinkCore>>)]>>,
>];

#[test]
fn a_plus_one_member_at_a_time_reads_above_it() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, PlusAboveOne, hlist![], |hlist_pat![
        mut v
    ]| {
        let mut seen = Vec::new();
        v.for_each_mut(|k, hlist_pat![mut plus]| {
            seen.push((*k, plus.take::<Under<ConnCore, Ref<LinkCore>>, _>().len()));
        });
        seen
    });
    assert_eq!(got, Some(vec![(PathKey(10), 3), (PathKey(20), 3)]));
}

/// The entry of a link's id among every entry of the connection.
struct ConnEntry;
impl Locator for ConnEntry {
    type Among = Under<ConnCore, Credit>;
    type Input = OwnKey<LinkId>;
    type Count = ExactlyOne;
    type Key = (PathKey, LinkId);
    fn pick<'t, P: Pos>(own: LinkId, among: &[(PathKey, LinkId)]) -> Option<(PathKey, LinkId)> {
        among.iter().find(|k| k.1 == own).copied()
    }
}

/// At each link, the credit of the entry the locator picks among the
/// entries of the connection, above the link, in a `+`.
type LocatorAbovePlus = HList![Under<
    ConnCore,
    At<LinkCore, needs![OwnKey<LinkId>, (Ref<LinkCore> + At<ConnEntry, HList![Ref<Credit>]>)]>,
>];

#[test]
fn a_locator_above_each_member_in_a_plus() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, LocatorAbovePlus, hlist![], |hlist_pat![
        mut v
    ]| {
        let mut seen = Vec::new();
        for (_, hlist_pat![k, plus]) in v.iter_mut() {
            let hlist_pat![c] = plus.take::<At<ConnEntry, HList![Ref<Credit>]>, _>();
            seen.push((*k, c.available));
        }
        seen
    });
    assert_eq!(
        got,
        Some(vec![(LinkId(1), 10), (LinkId(2), 20), (LinkId(3), 30)])
    );
}

/// By logical path, one at a time, every link core of the connection,
/// exclusive: the members lent one at a time may take what they share.
type MutAboveOne = HList![Under<
    ConnCore,
    OneAtATime<At<SentRecords, HList![Under<ConnCore, Mut<LinkCore>>]>>,
>];

#[test]
fn one_member_at_a_time_takes_above_it_exclusive() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, MutAboveOne, hlist![], |hlist_pat![
        mut v
    ]| {
        let mut seen = Vec::new();
        v.for_each_mut(|k, hlist_pat![mut all]| {
            for (_, l) in all.iter_mut() {
                l.rx += 1;
            }
            seen.push((*k, all.len()));
        });
        seen
    });
    assert_eq!(got, Some(vec![(PathKey(10), 3), (PathKey(20), 3)]));
    let got = lend!(
        &mut root,
        Conn1,
        HList![Under<ConnCore, Ref<LinkCore>>],
        hlist![],
        |hlist_pat![v]| v.iter().map(|(_, l)| l.rx).collect::<Vec<_>>()
    );
    assert_eq!(got, Some(vec![2, 2, 2]));
}

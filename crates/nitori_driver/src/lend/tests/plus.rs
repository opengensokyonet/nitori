//! `+`: branches that overlap, taken one after another.
use alloc::{vec, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::{
    lend::{Visit, lend_visit},
    need::{At, KeyedView, Members, Mut, Pos, Ref, Take, Under, View},
    needs,
};

/// Every link core of the connection, or the lend point's own.
type Links = needs![
    Ref<Keys>,
    (Under<ConnCore, Mut<LinkCore>> + Mut<LinkCore>),
    Mut<Planner>
];

#[test]
fn two_branches_that_overlap_taken_in_turn() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        Links,
        hlist![PathKey(10), LinkId(2)],
        |hlist_pat![keys, mut links, planner]| {
            {
                let mut all = links.take::<Under<ConnCore, Mut<LinkCore>>, _>();
                let _: &KeyedView<(PathKey, LinkId), &mut LinkCore> = &all;
                for (_, l) in all.members_mut() {
                    l.rx += 1;
                }
            }
            let mine = links.take::<Mut<LinkCore>, _>();
            mine.rx += 10;
            planner.next_pn += u64::from(keys.epoch);
            let all = Take::<Under<ConnCore, Mut<LinkCore>>, _>::take(&mut links);
            all.members().map(|(k, l)| (*k, l.rx)).collect::<Vec<_>>()
        }
    );
    assert_eq!(
        got,
        Some(vec![
            ((PathKey(10), LinkId(1)), 1),
            ((PathKey(10), LinkId(2)), 11),
            ((PathKey(20), LinkId(3)), 1)
        ])
    );
    let mut root = tree_two();
    let got = lend!(&mut root, LinkPath, Links, hlist![LinkId(1)], |hlist_pat![
        _keys,
        mut links,
        _planner
    ]| {
        links.take::<Mut<LinkCore>, _>().rx = 5;
        let all = links.take::<Under<ConnCore, Mut<LinkCore>>, _>();
        let _: &KeyedView<LinkId, &mut LinkCore> = &all;
        all.members().map(|(k, l)| (*k, l.rx)).collect::<Vec<_>>()
    });
    assert_eq!(got, Some(vec![(LinkId(1), 5), (LinkId(2), 0)]));
}

/// A branch that is a list, beside one that reads what it takes
/// exclusive.
type Count = needs![(HList![Mut<LinkCore>, Ref<Planner>] + Under<ConnCore, Ref<LinkCore>>)];

/// The lend point's link core counts the links; written for any structure.
struct CountLinks;
impl Visit<Count> for CountLinks {
    type Out = usize;
    fn visit<'v, P: Pos>(self, view: View<'v, Count, P>) -> usize {
        let hlist_pat![mut plus] = view;
        let n = plus
            .take::<Under<ConnCore, Ref<LinkCore>>, _>()
            .member_count();
        let hlist_pat![link, planner] = plus.take::<HList![Mut<LinkCore>, Ref<Planner>], _>();
        link.rx = n as u32 + planner.next_pn as u32;
        n
    }
}

#[test]
fn one_call_on_both_structures() {
    let mut root = tree_one();
    let at = hlist![PathKey(20), LinkId(3)];
    let n = lend_visit::<_, _, LinkPath, _, Count, _, _, _>(&mut root, &at, &mut (), CountLinks);
    assert_eq!(n, Some(3));
    let rx = lend!(&mut root, LinkPath, HList![Ref<LinkCore>], at, |hlist_pat![
        l
    ]| l.rx);
    assert_eq!(rx, Some(303));
    let mut root = tree_two();
    let at = hlist![LinkId(2)];
    let n = lend_visit::<_, _, LinkPath, _, Count, _, _, _>(&mut root, &at, &mut (), CountLinks);
    assert_eq!(n, Some(2));
    let rx = lend!(&mut root, LinkPath, HList![Ref<LinkCore>], at, |hlist_pat![
        l
    ]| l.rx);
    assert_eq!(rx, Some(202));
}

/// At each logical path, its sent records or the link cores below it.
type PerPath = HList![Under<
    ConnCore,
    At<SentRecords, needs![(Mut<SentRecords> + Under<SentRecords, Ref<LinkCore>>)]>,
>];

#[test]
fn a_plus_at_each_member() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, PerPath, hlist![], |hlist_pat![mut v]| {
        for (_, hlist_pat![plus]) in v.members_mut() {
            let n = plus
                .take::<Under<SentRecords, Ref<LinkCore>>, _>()
                .member_count();
            plus.take::<Mut<SentRecords>, _>().acked = n as u32;
        }
        v.members_mut()
            .map(|(k, hlist_pat![plus])| (*k, plus.take::<Mut<SentRecords>, _>().acked))
            .collect::<Vec<_>>()
    });
    assert_eq!(got, Some(vec![(PathKey(10), 2), (PathKey(20), 1)]));
}

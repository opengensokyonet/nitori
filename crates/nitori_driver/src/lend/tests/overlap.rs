//! Needs that share a component without overlapping.
use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::need::{Mut, Ref, Under};

#[test]
fn two_shared_needs_of_one_component() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Ref<SentRecords>, Under<ConnCore, Ref<SentRecords>>, Ref<Keys>, Ref<Keys>],
        hlist![PathKey(20), LinkId(3)],
        |hlist_pat![mine, all, a, b]| (mine.acked, all.len(), a.epoch + b.epoch)
    );
    assert_eq!(got, Some((0, 2, 14)));
}

#[test]
fn one_component_of_two_kinds() {
    let mut root = tree_two();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Mut<Planner>, Under<ConnCore, Mut<Credit>>, Mut<LinkCore>],
        hlist![LinkId(2)],
        |hlist_pat![planner, mut credit, link]| {
            planner.next_pn += 1;
            link.rx += 1;
            credit.get_mut(&LinkId(2)).map(|c| c.available)
        }
    );
    assert_eq!(got, Some(Some(20)));
}

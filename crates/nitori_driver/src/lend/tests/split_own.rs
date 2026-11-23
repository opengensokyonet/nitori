//! The members of the lend point's own branch, and the others.
use alloc::{vec, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::need::{KeyedView, Mut, Under};

#[test]
fn the_own_member_and_the_others() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Under<SentRecords, Mut<Credit>>],
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![mut v]| {
            let _: &KeyedView<LinkId, &mut Credit> = &v;
            let (mut own, rest) = v.split_own();
            let others: u32 = rest.iter().map(|(_, c)| c.available).sum();
            for (_, c) in own.iter_mut() {
                c.available += others;
            }
            (
                own.keys().copied().collect::<Vec<_>>(),
                rest.keys().copied().collect::<Vec<_>>(),
                v.get(&LinkId(1)).map(|c| c.available),
            )
        }
    );
    assert_eq!(got, Some((vec![LinkId(1)], vec![LinkId(2)], Some(30))));
    let mut root = tree_two();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Under<ConnCore, Mut<Credit>>],
        hlist![LinkId(2)],
        |hlist_pat![mut v]| {
            let (own, rest) = v.split_own();
            (
                own.keys().copied().collect::<Vec<_>>(),
                rest.keys().copied().collect::<Vec<_>>(),
            )
        }
    );
    assert_eq!(got, Some((vec![LinkId(2)], vec![LinkId(1)])));
}

#[test]
fn members_below_and_above_the_lend_point() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LogicalPath,
        HList![Under<ConnCore, Mut<LinkCore>>],
        hlist![PathKey(10)],
        |hlist_pat![mut v]| {
            let (own, rest) = v.split_own();
            (
                own.keys().copied().collect::<Vec<_>>(),
                rest.keys().copied().collect::<Vec<_>>(),
            )
        }
    );
    assert_eq!(
        got,
        Some((
            vec![(PathKey(10), LinkId(1)), (PathKey(10), LinkId(2))],
            vec![(PathKey(20), LinkId(3))]
        ))
    );
    let got = lend!(
        &mut root,
        Recovery,
        HList![Under<ConnCore, Mut<SentRecords>>],
        hlist![PathKey(20), LinkId(3)],
        |hlist_pat![mut v]| {
            let (own, rest) = v.split_own();
            (
                own.keys().copied().collect::<Vec<_>>(),
                rest.keys().copied().collect::<Vec<_>>(),
            )
        }
    );
    assert_eq!(got, Some((vec![PathKey(20)], vec![PathKey(10)])));
}

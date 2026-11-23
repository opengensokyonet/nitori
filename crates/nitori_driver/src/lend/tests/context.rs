//! The needs that read the lend point itself.
use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::{
    need::{At, OwnAddress, OwnKey, Under},
    structure::KeyAddress,
};

#[test]
fn own_key() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![OwnKey<LinkId>, At<SentRecords, HList![OwnKey<PathKey>]>],
        hlist![PathKey(10), LinkId(2)],
        |hlist_pat![link, hlist_pat![path]]| (link, path)
    );
    assert_eq!(got, Some((LinkId(2), PathKey(10))));
    let got = lend!(
        &mut root,
        Recovery,
        HList![OwnKey<()>],
        hlist![PathKey(20), LinkId(3)],
        |hlist_pat![k]| k
    );
    assert_eq!(got, Some(()));
    let got = lend!(
        &mut root,
        Conn1,
        HList![Under<ConnCore, At<SentRecords, HList![OwnKey<PathKey>]>>],
        hlist![],
        |hlist_pat![v]| v
            .members()
            .map(|(_, hlist_pat![k])| *k)
            .collect::<alloc::vec::Vec<_>>()
    );
    assert_eq!(got, Some(alloc::vec![PathKey(10), PathKey(20)]));
}

#[test]
fn own_address() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        Recovery,
        HList![OwnAddress],
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![a]| a
    );
    let want = KeyAddress::<Conn1, Conn1>::below::<LogicalPath, _, _>(
        PathKey(10),
        KeyAddress::<LogicalPath, LogicalPath>::below::<LinkPath, _, _>(
            LinkId(1),
            KeyAddress::<LinkPath, LinkPath>::child::<Recovery, _>(()),
        ),
    );
    let got: KeyAddress<Conn1, Recovery> = got.unwrap();
    assert_eq!(got, want);
    let mut root = tree_two();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![OwnAddress],
        hlist![LinkId(2)],
        |hlist_pat![a]| a
    );
    assert_eq!(
        got,
        Some(KeyAddress::<Conn2, Conn2>::child::<LinkPath, _>(LinkId(2)))
    );
    let got = lend!(&mut root, Conn2, HList![OwnAddress], hlist![], |hlist_pat![
        a
    ]| a);
    assert_eq!(got, Some(KeyAddress::<Conn2, Conn2>::here()));
}

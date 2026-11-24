//! The waker of a component: on the way up and of each member.
use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::{
    need::{Mut, OneAtATime, Under, WakerOf},
    needs,
};

/// The lend point's own waker beside its data, and every link's.
type Wakers = HList![
    Mut<Planner>,
    WakerOf<Planner>,
    WakerOf<SentRecords>,
    Under<ConnCore, WakerOf<Planner>>,
];

#[test]
fn up_and_of_each_member() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        Wakers,
        hlist![PathKey(10), LinkId(2)],
        |hlist_pat![planner, own, path, all]| {
            planner.next_pn += 1;
            let mine = all.get(&(PathKey(10), LinkId(2))).cloned();
            let other = all.get(&(PathKey(20), LinkId(3))).cloned();
            (mine == Some(own), other == Some(own), all.len(), path, own)
        }
    )
    .expect("the link path is there");
    let (mine, other, len, path, own) = got;
    assert_eq!((mine, other, len), (true, false, 3));
    assert_ne!(path, own);
    // The logical path's waker is the same from its other link, and
    // another logical path's differs.
    let again = lend!(
        &mut root,
        LinkPath,
        HList![WakerOf<SentRecords>],
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![w]| w
    );
    let elsewhere = lend!(
        &mut root,
        LinkPath,
        HList![WakerOf<SentRecords>],
        hlist![PathKey(20), LinkId(3)],
        |hlist_pat![w]| w
    );
    assert_eq!(again, Some(path));
    assert_ne!(elsewhere, Some(path));
    let mut root = tree_two();
    let got = lend!(&mut root, LinkPath, Wakers, hlist![LinkId(2)], |hlist_pat![
        _planner, own, _conn, all
    ]| {
        (all.get(&LinkId(2)) == Some(&own), all.len())
    });
    assert_eq!(got, Some((true, 2)));
}

/// Wakers in a branch of a `+` and one member at a time.
type Mixed = needs![
    (WakerOf<LinkCore> + Mut<LinkCore>),
    Under<ConnCore, OneAtATime<WakerOf<Planner>>>,
];

#[test]
fn in_branches_and_one_at_a_time() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        Mixed,
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![mut plus, mut each]| {
            let w = plus.take::<WakerOf<LinkCore>, _>();
            plus.take::<Mut<LinkCore>, _>().rx += 1;
            let mut n = 0;
            each.for_each_mut(|_, _w| n += 1);
            (w, n)
        }
    );
    let own = lend!(
        &mut root,
        LinkPath,
        HList![WakerOf<LinkCore>],
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![w]| w
    );
    assert_eq!(got.map(|(w, n)| (Some(w), n)), Some((own, 3)));
    let mut root = tree_two();
    let got = lend!(&mut root, LinkPath, Mixed, hlist![LinkId(1)], |hlist_pat![
        mut plus,
        each
    ]| {
        let _ = plus.take::<WakerOf<LinkCore>, _>();
        each.len()
    });
    assert_eq!(got, Some(2));
}

//! Custom locating: a lend point picked among candidates.
use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::need::{At, ExactlyOne, Locator, Mut, OneOrNone, OwnKey, Pos, Ref, Under, View};

/// The entry with the lend point's own link id, among those of its
/// logical path (structure one) or connection (structure two).
struct OwnEntry;
impl Locator for OwnEntry {
    type Among = Under<SentRecords, Credit>;
    type Input = HList![OwnKey<LinkId>, Ref<Keys>];
    type Count = ExactlyOne;
    type Key = LinkId;
    fn pick<'t, P: Pos>(input: View<'t, Self::Input, P>, among: &[LinkId]) -> Option<LinkId> {
        let hlist_pat![own, keys] = input;
        assert!(keys.epoch > 0);
        among.iter().find(|k| **k == own).copied()
    }
}

/// The entry of the next link id, which may not be there.
struct NextEntry;
impl Locator for NextEntry {
    type Among = Under<SentRecords, Credit>;
    type Input = OwnKey<LinkId>;
    type Count = OneOrNone;
    type Key = LinkId;
    fn pick<'t, P: Pos>(own: LinkId, among: &[LinkId]) -> Option<LinkId> {
        let _ = among;
        Some(LinkId(own.0 + 1))
    }
}

/// An entry ten ids on, which is never there, under the contract that it
/// is.
struct FarEntry;
impl Locator for FarEntry {
    type Among = Under<SentRecords, Credit>;
    type Input = OwnKey<LinkId>;
    type Count = ExactlyOne;
    type Key = LinkId;
    fn pick<'t, P: Pos>(own: LinkId, _: &[LinkId]) -> Option<LinkId> {
        Some(LinkId(own.0 + 10))
    }
}

/// The lend point's own entry: its credit taken, beside its own planner.
type Own = HList![Mut<Planner>, At<OwnEntry, HList![Mut<Credit>, Ref<LinkLimit>]>];

#[test]
fn exactly_one_picks_its_member() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        Own,
        hlist![PathKey(10), LinkId(2)],
        |hlist_pat![planner, hlist_pat![credit, limit]]| {
            let _: &mut Credit = credit;
            credit.available += 1;
            planner.next_pn += 1;
            (credit.available, limit.max)
        }
    );
    assert_eq!(got, Some((21, 200)));
    let mut root = tree_two();
    let got = lend!(&mut root, LinkPath, Own, hlist![LinkId(1)], |hlist_pat![
        _planner,
        hlist_pat![credit, limit]
    ]| (
        credit.available,
        limit.max
    ));
    assert_eq!(got, Some((10, 100)));
}

type Next = HList![At<NextEntry, HList![Mut<Credit>]>];

#[test]
fn one_or_none_gives_none() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        Next,
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![next]| {
            let next: Option<HList![&mut Credit]> = next;
            next.map(|hlist_pat![c]| c.available)
        }
    );
    assert_eq!(got, Some(Some(20)));
    // Link 3 is in the other logical path: the next entry of path 10 is
    // not among the candidates of path 10.
    let got = lend!(
        &mut root,
        LinkPath,
        Next,
        hlist![PathKey(10), LinkId(2)],
        |hlist_pat![next]| next.is_none()
    );
    assert_eq!(got, Some(true));
    let mut root = tree_two();
    let got = lend!(&mut root, LinkPath, Next, hlist![LinkId(2)], |hlist_pat![
        next
    ]| next
        .is_none());
    assert_eq!(got, Some(true));
}

#[test]
#[should_panic(expected = "FarEntry")]
fn exactly_one_that_picks_none_panics() {
    let mut root = tree_one();
    let _ = lend!(
        &mut root,
        LinkPath,
        HList![At<FarEntry, HList![Ref<Credit>]>],
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![_c]| ()
    );
}

#[test]
#[should_panic(expected = "picked none of its candidates")]
fn exactly_one_that_picks_none_panics_on_structure_two() {
    let mut root = tree_two();
    let _ = lend!(
        &mut root,
        LinkPath,
        HList![At<FarEntry, HList![Ref<Credit>]>],
        hlist![LinkId(1)],
        |hlist_pat![_c]| ()
    );
}

/// At each logical path, the entry of link 1, where there is one.
struct FirstEntry;
impl Locator for FirstEntry {
    type Among = Under<SentRecords, Credit>;
    type Input = HList![];
    type Count = OneOrNone;
    type Key = LinkId;
    fn pick<'t, P: Pos>(_: View<'t, Self::Input, P>, among: &[LinkId]) -> Option<LinkId> {
        among.iter().find(|k| k.0 == 1).copied()
    }
}

#[test]
fn a_locator_at_each_member() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        Conn1,
        HList![Under<ConnCore, At<SentRecords, HList![At<FirstEntry, HList![Mut<Credit>]>]>>],
        hlist![],
        |hlist_pat![mut v]| {
            v.iter_mut()
                .map(|(p, hlist_pat![e])| (*p, e.as_ref().map(|c| c.head.available)))
                .collect::<alloc::vec::Vec<_>>()
        }
    );
    assert_eq!(
        got,
        Some(alloc::vec![(PathKey(10), Some(10)), (PathKey(20), None)])
    );
}

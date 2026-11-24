//! One `#[call]` function written for any structure, composed on both and
//! polled once on each: a send beside a branch whose reaction point fires
//! the reactions of two parts.
use alloc::{string::String, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::{Q1, Q2, calls::Sending, cx, noop};
use crate::{
    call::{poll_call, start_calls},
    lend::tests::{
        fixtures::{
            Congestion, LinkCore, LinkId, LinkPath, PathKey, SentRecords, lend, same_on_both,
            tree_one_as_two, tree_two,
        },
        parts::take_log,
    },
    need::{At, Members, Ref, Under},
};

/// What a send did: the poll, the log, the link's count with its
/// recovery's congestion, and the sent records.
type Sent = (
    Option<bool>,
    Vec<String>,
    Option<(u32, Vec<u32>)>,
    Option<Vec<u64>>,
);

macro_rules! after_send {
    ($root:expr, $at:expr) => {
        (
            lend!(
                $root,
                LinkPath,
                HList![Ref<LinkCore>, Under<LinkCore, Ref<Congestion>>],
                $at,
                |hlist_pat![link, congestion]| (
                    link.rx,
                    Members::iter(&congestion).map(|(_, c)| c.in_flight).collect()
                )
            ),
            lend!(
                $root,
                LinkPath,
                HList![At<SentRecords, Ref<SentRecords>>],
                $at,
                |hlist_pat![s]| s.sent.clone()
            ),
        )
    };
}

#[test]
fn one_function_on_both() {
    let mut root = tree_one_as_two();
    let mut cx1 = cx::<Q1>();
    let at = hlist![PathKey(10), LinkId(1)];
    let hlist_pat![sending, _, _, _, _, _] = start_calls::<Q1, LinkPath, _>(&mut 5);
    take_log();
    let mut sending = core::pin::pin!(sending);
    let polled = poll_call::<Q1, LinkPath, Sending, _, _>(
        &mut root,
        &at,
        &mut cx1,
        sending.as_mut(),
        &mut noop(),
    )
    .map(|p| p.is_ready());
    let log = take_log();
    let (link, sent) = after_send!(&mut root, at);
    let one: Sent = (polled, log, link, sent);

    let mut root = tree_two();
    let mut cx2 = cx::<Q2>();
    let at = hlist![LinkId(1)];
    let hlist_pat![sending, _, _, _, _, _] = start_calls::<Q2, LinkPath, _>(&mut 5);
    take_log();
    let mut sending = core::pin::pin!(sending);
    let polled = poll_call::<Q2, LinkPath, Sending, _, _>(
        &mut root,
        &at,
        &mut cx2,
        sending.as_mut(),
        &mut noop(),
    )
    .map(|p| p.is_ready());
    let log = take_log();
    let (link, sent) = after_send!(&mut root, at);
    let two: Sent = (polled, log, link, sent);

    same_on_both(
        one,
        two,
        (
            Some(true),
            [
                "RA1 count=3 congestion=1",
                "RA2 epoch=3 sent=1 count=3",
                "sent 100",
            ]
            .map(String::from)
            .to_vec(),
            Some((3, alloc::vec![3])),
            Some(alloc::vec![100]),
        ),
    );
}

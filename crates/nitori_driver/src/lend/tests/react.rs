//! Reaction points on both structures, fired by code generic over its
//! position: the reactions of two parts and not of the third, a chain of
//! two points, a point beside a branch that overlaps a reaction, which
//! kinds' reactions a point takes, and the driver's two points.
use alloc::{string::String, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::{
    fixtures::{lend, *},
    parts::*,
};
use crate::{
    lend::{
        Visit,
        entry::{node_ending, node_started},
        lend_visit,
    },
    need::{At, Cap, Members, Mut, Plus, Pos, Ref, Under, View},
    needs,
};

fn lines(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| String::from(*s)).collect()
}

type SendA = needs![
    Mut<Planner>,
    (At<SentRecords, Mut<SentRecords>> + Cap<PacketsSent>)
];
/// A send: the next packet number, recorded, then the reactions to it.
struct Send(u32);
impl Visit<SendA> for Send {
    type Out = u64;
    fn visit<'v, P: Pos>(self, view: View<'v, SendA, P>) -> u64 {
        let hlist_pat![planner, mut plus] = view;
        let pn = planner.next_pn;
        planner.next_pn += 1;
        plus.take::<At<SentRecords, Mut<SentRecords>>, _>()
            .sent
            .push(pn);
        plus.take::<Cap<PacketsSent>, _>()
            .fire(&SentBatch { count: self.0 });
        pn
    }
}

/// What a send did: its packet number, the log, the link's count and its
/// recovery's congestion.
type Sent = (Option<u64>, Vec<String>, Option<(u32, Vec<u32>)>);

macro_rules! after_send {
    ($root:expr, $at:expr) => {
        lend!(
            $root,
            LinkPath,
            HList![Ref<LinkCore>, Under<LinkCore, Ref<Congestion>>],
            $at,
            |hlist_pat![link, congestion]| (
                link.rx,
                Members::iter(&congestion).map(|(_, c)| c.in_flight).collect()
            )
        )
    };
}

/// a: the core's reaction at the link path and the loss part's at the
/// connection run, in the order of the parts; the watch's, to another
/// point, does not.
#[test]
fn two_parts_react() {
    let mut root = tree_one_as_two();
    let at = hlist![PathKey(10), LinkId(1)];
    let pn = lend_visit::<_, _, LinkPath, _, SendA, _, _, _>(&mut root, &at, &mut (), Send(3));
    let one: Sent = (pn, take_log(), after_send!(&mut root, at));
    let mut root = tree_two();
    let at = hlist![LinkId(1)];
    let pn = lend_visit::<_, _, LinkPath, _, SendA, _, _, _>(&mut root, &at, &mut (), Send(3));
    let two: Sent = (pn, take_log(), after_send!(&mut root, at));
    same_on_both(
        one,
        two,
        (
            Some(100),
            lines(&["RA1 count=3 congestion=1", "RA2 epoch=3 sent=1 count=3"]),
            Some((3, alloc::vec![3])),
        ),
    );
}

type Chain = HList![Cap<P1>];
/// c: the first point's reaction fires the second.
struct FireFirst;
impl Visit<Chain> for FireFirst {
    type Out = ();
    fn visit<'v, P: Pos>(self, view: View<'v, Chain, P>) {
        let hlist_pat![mut first] = view;
        first.fire(7);
    }
}

#[test]
fn chained() {
    let mut root = tree_one_as_two();
    let at = hlist![PathKey(10), LinkId(2)];
    lend_visit::<_, _, LinkPath, _, Chain, _, _, _>(&mut root, &at, &mut (), FireFirst);
    let pn = lend!(&mut root, LinkPath, HList![Ref<Planner>], at, |hlist_pat![
        p
    ]| p
        .next_pn);
    let one = (take_log(), pn);
    let mut root = tree_two();
    let at = hlist![LinkId(2)];
    lend_visit::<_, _, LinkPath, _, Chain, _, _, _>(&mut root, &at, &mut (), FireFirst);
    let pn = lend!(&mut root, LinkPath, HList![Ref<Planner>], at, |hlist_pat![
        p
    ]| p
        .next_pn);
    let two = (take_log(), pn);
    same_on_both(one, two, (lines(&["RC1 7", "RC2 70"]), Some(270)));
}

type SendE = Plus<HList![Under<ConnCore, Mut<Congestion>>, Cap<PacketsSent>]>;
/// e: every congestion first, then the reactions, which take the lend
/// point's congestion again.
struct Drain(u32);
impl Visit<SendE> for Drain {
    type Out = usize;
    fn visit<'v, P: Pos>(self, mut view: View<'v, SendE, P>) -> usize {
        let n = {
            let mut every = view.take::<Under<ConnCore, Mut<Congestion>>, _>();
            for (_, c) in every.iter_mut() {
                c.in_flight += 100;
            }
            every.len()
        };
        view.take::<Cap<PacketsSent>, _>()
            .fire(&SentBatch { count: self.0 });
        n
    }
}

#[test]
fn beside_a_branch() {
    let mut root = tree_one_as_two();
    let at = hlist![PathKey(10), LinkId(1)];
    let n = lend_visit::<_, _, LinkPath, _, SendE, _, _, _>(&mut root, &at, &mut (), Drain(2));
    let one: Sent = (n.map(|n| n as u64), take_log(), after_send!(&mut root, at));
    let mut root = tree_two();
    let at = hlist![LinkId(1)];
    let n = lend_visit::<_, _, LinkPath, _, SendE, _, _, _>(&mut root, &at, &mut (), Drain(2));
    let two: Sent = (n.map(|n| n as u64), take_log(), after_send!(&mut root, at));
    same_on_both(
        one,
        two,
        (
            Some(1),
            lines(&["RA1 count=2 congestion=1", "RA2 epoch=3 sent=0 count=2"]),
            Some((2, alloc::vec![102])),
        ),
    );
}

type ProbeIt = HList![Cap<Probe>];
/// Fire the probe with a number.
struct FireProbe(u32);
impl Visit<ProbeIt> for FireProbe {
    type Out = ();
    fn visit<'v, P: Pos>(self, view: View<'v, ProbeIt, P>) {
        let hlist_pat![mut probe] = view;
        probe.fire(self.0);
    }
}

/// Card 9: at a link path, the reactions of the connection's shares and
/// of the link path's, of each part, the kinds from the root down; at the
/// connection, the connection's only.
#[test]
fn which_kinds_react() {
    let mut root = tree_one_as_two();
    lend_visit::<_, _, LinkPath, _, ProbeIt, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(1)],
        &mut (),
        FireProbe(5),
    );
    lend_visit::<_, _, Conn1, _, ProbeIt, _, _, _>(&mut root, &hlist![], &mut (), FireProbe(6));
    let one = take_log();
    let mut root = tree_two();
    lend_visit::<_, _, LinkPath, _, ProbeIt, _, _, _>(
        &mut root,
        &hlist![LinkId(1)],
        &mut (),
        FireProbe(5),
    );
    lend_visit::<_, _, Conn2, _, ProbeIt, _, _, _>(&mut root, &hlist![], &mut (), FireProbe(6));
    let two = take_log();
    same_on_both(
        one,
        two,
        lines(&[
            "core@conn 5 1",
            "core@link 5 1",
            "loss@conn 5 11",
            "loss@link 5 1",
            "core@conn 6 12",
            "loss@conn 6 22",
        ]),
    );
}

type SurveyIt = HList![Cap<Survey>];
/// Fire the survey with a number.
struct FireSurvey(u32);
impl Visit<SurveyIt> for FireSurvey {
    type Out = ();
    fn visit<'v, P: Pos>(self, view: View<'v, SurveyIt, P>) {
        let hlist_pat![mut survey] = view;
        survey.fire(self.0);
    }
}

/// A reaction taking the members of the connection, of a kind on the route
/// to the lend point too, from a point lent outside a `+`: every member,
/// not the one on the route alone.
#[test]
fn a_reaction_takes_every_member_beside_the_route() {
    let mut root = tree_one_as_two();
    let at = hlist![PathKey(10), LinkId(1)];
    lend_visit::<_, _, LinkPath, _, SurveyIt, _, _, _>(&mut root, &at, &mut (), FireSurvey(4));
    let one = (take_log(), after_send!(&mut root, at));
    let mut root = tree_two();
    let at = hlist![LinkId(1)];
    lend_visit::<_, _, LinkPath, _, SurveyIt, _, _, _>(&mut root, &at, &mut (), FireSurvey(4));
    let two = (take_log(), after_send!(&mut root, at));
    same_on_both(
        one,
        two,
        (lines(&["survey@link 4 2 300 2"]), Some((1, alloc::vec![0]))),
    );
}

/// The driver's points at link 2: started takes the reactions of the
/// connection and of the link path, ending only the connection's.
#[test]
fn started_and_ending() {
    let mut root = tree_one_as_two();
    let at = hlist![PathKey(10), LinkId(2)];
    let started = node_started::<_, _, LinkPath, _, _, _>(&mut root, &at, &mut ());
    let ending = node_ending::<_, _, LinkPath, _, _, _>(&mut root, &at, &mut ());
    let absent = node_started::<_, _, LinkPath, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(9)],
        &mut (),
    );
    let core = lend!(
        &mut root,
        Conn1,
        HList![Ref<ConnCore>],
        hlist![],
        |hlist_pat![c]| c.rx
    );
    let one = (started, ending, absent, take_log(), core);
    let mut root = tree_two();
    let at = hlist![LinkId(2)];
    let started = node_started::<_, _, LinkPath, _, _, _>(&mut root, &at, &mut ());
    let ending = node_ending::<_, _, LinkPath, _, _, _>(&mut root, &at, &mut ());
    let absent = node_started::<_, _, LinkPath, _, _, _>(&mut root, &hlist![LinkId(9)], &mut ());
    let core = lend!(
        &mut root,
        Conn2,
        HList![Ref<ConnCore>],
        hlist![],
        |hlist_pat![c]| c.rx
    );
    let two = (started, ending, absent, take_log(), core);
    same_on_both(
        one,
        two,
        (
            Some(()),
            Some(()),
            None,
            lines(&["started@conn 2", "started@link 200", "ending@conn 0"]),
            Some(101),
        ),
    );
}

//! The kinds of call of the link paths, written for any structure: a send
//! with a reaction point beside a branch, two calls that take what is left
//! of the start, a call that waits for a gate outside the tree, and a call
//! that asks the connection to close.
extern crate std;

use alloc::format;
use core::{cell::Cell, future::poll_fn, task::Poll};

use frunk_core::{HList, hlist_pat};
use nitori_call::{LentAccess, call};

use super::super::{CallFamily, CallKind, Composition, LoanNeeds, RootOf};
use crate::{
    lend::tests::{
        fixtures::{CloseConn, LinkCore, LinkId, Planner, SentRecords},
        parts::{PacketsSent, SentBatch, log},
    },
    need::{At, Cap, Mut, OwnKey, Requests},
    needs,
    request::NodeRequest,
    structure::{KeyAddress, NodeKind},
};

/// A send: the next packet number, recorded in the sent records, then the
/// reactions to it, the core's at the link and the loss part's at the
/// connection.
impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Sending {
    type Needs = needs![
        Mut<Planner>,
        (At<SentRecords, Mut<SentRecords>> + Cap<PacketsSent>)
    ];
    type Op = Self;
    fn start(_: &mut K::Start) -> Self {
        Sending::new()
    }
}
#[call]
pub async fn sending<W: CallFamily<C = Sending>>(host: &mut impl LentAccess<Family = W>) {
    host.with(|mut view| {
        let hlist_pat![planner, plus] = view.needs();
        let pn = planner.next_pn;
        planner.next_pn += 1;
        plus.take::<At<SentRecords, Mut<SentRecords>>, _>()
            .sent
            .push(pn);
        plus.take::<Cap<PacketsSent>, _>()
            .fire(&SentBatch { count: 3 });
        log(format!("sent {pn}"));
    });
}

/// A call that keeps a budget: what is left of its link's start, after
/// the components took theirs; it takes ten of it for the calls after it.
impl<Q: Composition, K: NodeKind<Start = u64>> CallKind<Q, K> for BudgetA {
    type Needs = HList![Mut<Planner>];
    type Op = Self;
    fn start(start: &mut u64) -> Self {
        let budget = *start;
        *start += 10;
        log(format!("start a {budget}"));
        BudgetA::new(budget)
    }
}
#[call]
pub async fn budget_a<W: CallFamily<C = BudgetA, K: NodeKind<Start = u64>>>(
    host: &mut impl LentAccess<Family = W>,
    budget: u64,
) {
    host.with(|mut view| {
        let hlist_pat![planner] = view.needs();
        planner.next_pn += budget;
    });
}

/// The loss part's call with a budget, after the core's.
impl<Q: Composition, K: NodeKind<Start = u64>> CallKind<Q, K> for BudgetB {
    type Needs = HList![];
    type Op = Self;
    fn start(start: &mut u64) -> Self {
        log(format!("start b {start}"));
        BudgetB::new(*start)
    }
}
#[call]
pub async fn budget_b<W: CallFamily<C = BudgetB, K: NodeKind<Start = u64>>>(
    host: &mut impl LentAccess<Family = W>,
    budget: u64,
) {
    host.with(|_| log(format!("budget b {budget}")));
}

std::thread_local! {
    static GATE: Cell<bool> = const { Cell::new(false) };
}
/// Open or close the gate the waiting call waits for.
pub fn set_gate(open: bool) {
    GATE.with(|g| g.set(open));
}

/// A call that waits for the gate, outside the tree, then counts a
/// hundred in its link's core and returns.
impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Waiting {
    type Needs = HList![Mut<LinkCore>];
    type Op = Self;
    fn start(_: &mut K::Start) -> Self {
        Waiting::new()
    }
}
#[call]
pub async fn waiting<W: CallFamily<C = Waiting>>(host: &mut impl LentAccess<Family = W>) {
    poll_fn(|_| {
        if GATE.with(Cell::get) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    host.with(|mut view| {
        let hlist_pat![core] = view.needs();
        core.rx += 100;
    });
}

/// What a closing call asks of the composition: a root that can be asked
/// to close.
pub trait ClosesConn: Composition<Root: NodeKind<Request = CloseConn>> {}
impl<Q: Composition<Root: NodeKind<Request = CloseConn>>> ClosesConn for Q {}

/// A call that asks the connection to close twice, for its link's id and
/// for a hundred more.
impl<Q: ClosesConn, K: NodeKind> CallKind<Q, K> for Closing {
    type Needs = HList![OwnKey<LinkId>, Requests<NodeRequest<RootOf<Q>, RootOf<Q>>>];
    type Op = Self;
    fn start(_: &mut K::Start) -> Self {
        Closing::new()
    }
}
#[call]
pub async fn closing<W: CallFamily<C = Closing, Q: ClosesConn>>(
    host: &mut impl LentAccess<Family = W>,
) {
    host.with(|mut view| {
        let hlist_pat![own, closes] = view.needs();
        closes.push(NodeRequest::new(KeyAddress::here(), CloseConn(own.0)));
        closes.push(NodeRequest::new(KeyAddress::here(), CloseConn(own.0 + 100)));
    });
}

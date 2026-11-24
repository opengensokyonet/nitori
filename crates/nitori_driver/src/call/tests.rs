//! Calls composed on the two structures of the lending's checks: one
//! `#[call]` function on both, the start a new node's calls take, a call
//! that waits, a contributed capability through a bound of the
//! composition, requests put in, and the other entries of the pack.
use core::{
    task::{Context, Waker},
    time::Duration,
};

use frunk_core::HList;

use super::{Composition, ContextOf, context};
use crate::{
    composition,
    lend::tests::fixtures::{Conn1, Conn2, Parts, TestClock},
};

mod calls;
mod contributed;
mod entries;
mod generic;
mod ready;
mod requests;
mod start;

/// Structure one, composed.
pub struct Q1;
composition!(impl for Q1: Conn1, Parts, TestClock, u8);
/// Structure two, composed.
pub struct Q2;
composition!(impl for Q2: Conn2, Parts, TestClock, u8);

/// The core's calls at a link path, in the order they start.
pub type CoreLinkCalls = HList![
    calls::Sending,
    calls::BudgetA,
    calls::Waiting,
    calls::Closing,
    contributed::Receiving,
];
/// The loss part's calls at a link path.
pub type LossLinkCalls = HList![calls::BudgetB];

/// The context of a lending under `Q`, read at one second, with empty
/// boxes.
pub fn cx<Q: Composition<Clock = TestClock>>() -> ContextOf<Q> {
    context::<Q>(
        TestClock {
            last: Duration::from_secs(10),
        },
        Duration::from_secs(1),
    )
}

/// A context of a poll whose waker does nothing.
pub fn noop() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

//! The timer call.

use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use nitori_call::{LentAccess, call};
use nitori_clock::MonotonicClock;
use nitori_driver::{
    call::{CallFamily, CallKind, Composition, LoanHost, LoanNeeds},
    frunk_core::{HList, hlist_pat},
    need::{Clock, Mut, Wakes},
    structure::NodeKind,
};

use crate::TimerTable;

/// The timer call at the root, for the clock `Ck` and the priority `Pr`
/// of the composition: it wakes what is due, and sleeps until the next
/// instant, or until a `set` moves it earlier.
impl<Q, K, Ck, Pr> CallKind<Q, K> for FireTimers<Ck, Pr>
where
    Q: Composition<Clock = Ck, Priority = Pr>,
    K: NodeKind,
    Ck: MonotonicClock + 'static,
    Pr: Ord + Copy + 'static,
{
    type Needs = HList![Mut<TimerTable<Ck, Pr>>, Clock<Ck>, Wakes<Pr>];
    type Op = Self;
    fn start(_: &mut K::Start) -> Self {
        FireTimers::new()
    }
}

/// How a sleep of the timer call ended.
enum Woken<I> {
    /// The timer for this instant ended.
    Timer(I),
    /// A `set` moved the earliest instant earlier.
    Moved,
}

/// The timer call. Each round it wakes, at their priorities, the entries
/// due, arms for the earliest instant left with the highest priority of
/// the entries, and sleeps until the timer ends or a `set` moves the
/// earliest instant.
///
/// An entry is due when its instant is at most the instant the lending
/// read, or, after a timer ended, the instant the timer was for: a clock
/// whose timer ends before its reading gets there does not make the call
/// arm the same timer again and again.
#[call]
pub async fn fire_timers<Ck, Pr, W>(host: &mut impl LentAccess<Family = W>)
where
    Ck: MonotonicClock + 'static,
    Pr: Ord + Copy + 'static,
    W: CallFamily<C = FireTimers<Ck, Pr>, Q: Composition<Clock = Ck, Priority = Pr>>,
{
    let mut ended = None;
    loop {
        let armed = host.with(|mut view| {
            let (armed, p) = {
                let hlist_pat![table, clock, wakes] = view.needs();
                let now = clock.now();
                let by = match ended {
                    Some(at) if at > now => at,
                    _ => now,
                };
                table.fire(by, *wakes);
                match table.arm() {
                    Some((at, p)) => (Some((at, clock.sleep_until(at))), Some(p)),
                    None => (None, None),
                }
            };
            if let Some(p) = p {
                view.set_wake_priority(p);
            }
            armed
        });
        let (at, sleep) = armed.unzip();
        let mut sleep = pin!(sleep);
        let woken = poll_fn(|cx| {
            let moved = host.with(|mut view| {
                let hlist_pat![table, _, _] = view.needs();
                table.register(cx.waker())
            });
            if moved {
                return Poll::Ready(Woken::Moved);
            }
            match (sleep.as_mut().as_pin_mut(), at) {
                (Some(s), Some(at)) => s.poll(cx).map(|()| Woken::Timer(at)),
                _ => Poll::Pending,
            }
        })
        .await;
        ended = match woken {
            Woken::Timer(at) => Some(at),
            Woken::Moved => None,
        };
    }
}

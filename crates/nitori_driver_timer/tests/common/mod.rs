//! The structure the tests run: a top node and leaves by number,
//! each leaf with an alarm that notes its wakes; a clock that moves only
//! when told to, and notes every timer it makes.
#![allow(dead_code)]
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
    time::Duration,
    vec::Vec,
};

use nitori_driver::{
    component::{ArgsOf, Component, EndCause, PartList},
    driver::{Edit, HandlerFor},
    frunk_core::{HList, hlist_pat},
    id::Numbered,
    need::{Mut, OwnKey, Pos, View, WakerOf},
    request::HandleRequest,
    structure::{Child, NodeKind, Share, SlotVec},
};

std::thread_local! {
    static LOG: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}
/// The leaves woken since the last time, in order, leaving none.
pub fn woken() -> Vec<u32> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

std::thread_local! {
    static CLEAR_ON_END: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static NEXT: RefCell<Vec<Duration>> = const { RefCell::new(Vec::new()) };
}
/// Whether an alarm clears its own instant when its leaf ends (off at the
/// start of every test).
pub fn clear_on_end(on: bool) {
    CLEAR_ON_END.with(|c| c.set(on));
}
/// The instants an alarm sets for itself, one in each of its wakes, in
/// order.
pub fn next_instants(list: &[Duration]) {
    NEXT.with(|n| *n.borrow_mut() = list.to_vec());
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Low,
    Mid,
    High,
}
impl nitori_driver_timer::TimerCallPriority for Level {
    const TIMER_CALL: Self = Level::High;
}

/// A clock that moves only when told to.
#[derive(Clone, Default)]
pub struct FakeClock(pub Arc<Mutex<State>>);
#[derive(Default)]
pub struct State {
    pub now: Duration,
    /// Every instant a timer was made for, in order.
    pub made: Vec<Duration>,
    /// The wakers of the timers that wait.
    pub waiting: Vec<(Duration, Waker)>,
    /// Timers ended before the reading got to them, each to be polled
    /// once.
    pub ended: Vec<Duration>,
}
impl FakeClock {
    /// Move to `to`, waking the timers that end by then.
    pub fn advance(&self, to: Duration) {
        let due: Vec<Waker> = {
            let mut s = self.0.lock().unwrap();
            s.now = to;
            let (due, rest) = std::mem::take(&mut s.waiting)
                .into_iter()
                .partition::<Vec<_>, _>(|(at, _)| *at <= to);
            s.waiting = rest;
            due.into_iter().map(|(_, w)| w).collect()
        };
        due.into_iter().for_each(Waker::wake);
    }
    /// End the waiting timers without moving the reading, as a clock may
    /// whose timer is coarser than its reading.
    pub fn end_early(&self) {
        let due: Vec<Waker> = {
            let mut s = self.0.lock().unwrap();
            let waiting = std::mem::take(&mut s.waiting);
            s.ended.extend(waiting.iter().map(|(at, _)| *at));
            waiting.into_iter().map(|(_, w)| w).collect()
        };
        due.into_iter().for_each(Waker::wake);
    }
    /// The instants timers were made for since the last time.
    pub fn made(&self) -> Vec<Duration> {
        std::mem::take(&mut self.0.lock().unwrap().made)
    }
}
pub struct FakeSleep {
    clock: FakeClock,
    at: Duration,
}
impl Future for FakeSleep {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut s = self.clock.0.lock().unwrap();
        if s.now >= self.at {
            return Poll::Ready(());
        }
        if let Some(i) = s.ended.iter().position(|at| *at == self.at) {
            s.ended.swap_remove(i);
            return Poll::Ready(());
        }
        let at = self.at;
        s.waiting.push((at, cx.waker().clone()));
        Poll::Pending
    }
}
impl nitori_clock::MonotonicClock for FakeClock {
    type Instant = Duration;
    type Sleep = FakeSleep;
    fn now(&self) -> Duration {
        self.0.lock().unwrap().now
    }
    fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> {
        at.checked_add(by)
    }
    fn sleep_until(&self, at: Duration) -> FakeSleep {
        self.0.lock().unwrap().made.push(at);
        FakeSleep {
            clock: self.clone(),
            at,
        }
    }
    fn timer_granularity(&self) -> Duration {
        Duration::ZERO
    }
}
/// The root: leaves by number; it is asked to add one.
#[derive(Numbered)]
pub struct Top;
impl NodeKind for Top {
    type Children = HList![Child<Leaf, SlotVec<u32>>];
    type Request = u32;
    type Handler = Adding;
}
#[derive(Numbered)]
pub struct Leaf;
impl NodeKind for Leaf {
    type Children = HList![];
}
/// Adds the leaf of the number it is asked.
pub struct Adding;
impl HandleRequest<Top, Level> for Adding {
    type Needs = HList![];
    fn handle<P: HandlerFor<Top, Self, Level>>(mut edit: Edit<'_, Top, P>, key: u32) {
        if key >= 1000 {
            let _ = edit.end::<Leaf, _>(&(key - 1000), ());
        } else {
            let _ = edit.add::<Leaf, _>(key, (), Level::Low);
        }
    }
}

/// A leaf's alarm: it notes its wakes.
#[derive(Numbered)]
pub struct Alarm(pub u32);
impl<L: PartList<L>> Component<Run, L, Leaf> for Alarm {
    type BuildNeeds = HList![OwnKey<u32>];
    fn build<P: Pos>(_: &mut (), _: &ArgsOf<L>, view: View<'_, Self::BuildNeeds, P>) -> Self {
        let hlist_pat![key] = view;
        Alarm(key)
    }
    type Needs = HList![
        Mut<nitori_driver_timer::TimerTable<FakeClock, Level>>,
        WakerOf<Alarm>
    ];
    fn wake<P: Pos>(view: nitori_driver::component::ComponentView<'_, Self, Self::Needs, P>) {
        let hlist_pat![me, table, waker] = view;
        LOG.with(|l| l.borrow_mut().push(me.0));
        let next = NEXT.with(|n| {
            let mut n = n.borrow_mut();
            (!n.is_empty()).then(|| n.remove(0))
        });
        if let Some(at) = next {
            table.set(at, &waker, Level::Low);
        }
    }
    fn end<P: Pos>(
        view: nitori_driver::component::ComponentView<'_, Self, Self::Needs, P>,
        _: EndCause<'_, ()>,
    ) {
        let hlist_pat![_, table, waker] = view;
        if CLEAR_ON_END.with(|c| c.get()) {
            table.clear(&waker);
        }
    }
}

/// The part with the alarms.
pub struct Run;
impl<L> nitori_driver::component::Part<L> for Run {
    type Args = ();
}
impl Share<Top> for Run {
    type Data = HList![];
}
impl Share<Leaf> for Run {
    type Data = HList![Alarm];
}

/// A waker that counts its wakes.
#[derive(Default)]
pub struct Count(pub std::sync::atomic::AtomicUsize);
impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
pub fn outer() -> (Arc<Count>, Waker) {
    let c = Arc::new(Count::default());
    (c.clone(), Waker::from(c))
}

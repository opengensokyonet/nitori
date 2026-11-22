//! A clock the test moves by hand.
use std::{
    cell::{Cell, RefCell},
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
    time::Duration,
};

use nitori_clock::MonotonicClock;

/// The instant is the time since the start.
#[derive(Clone, Default)]
pub struct TestClock {
    now: Rc<Cell<Duration>>,
    sleepers: Rc<RefCell<Vec<Waker>>>,
}
impl TestClock {
    /// Move the clock to `to` and wake every sleep.
    pub fn set(&self, to: Duration) {
        self.now.set(to);
        for w in self.sleepers.borrow_mut().drain(..) {
            w.wake();
        }
    }
}
/// A sleep on the test clock.
pub struct TestSleep {
    clock: TestClock,
    at: Duration,
}
impl Future for TestSleep {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.clock.now.get() >= self.at {
            Poll::Ready(())
        } else {
            self.clock.sleepers.borrow_mut().push(cx.waker().clone());
            Poll::Pending
        }
    }
}
impl MonotonicClock for TestClock {
    type Instant = Duration;
    type Sleep = TestSleep;
    fn now(&self) -> Duration {
        self.now.get()
    }
    fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> {
        at.checked_add(by)
    }
    fn sleep_until(&self, at: Duration) -> TestSleep {
        TestSleep {
            clock: self.clone(),
            at,
        }
    }
    fn timer_granularity(&self) -> Duration {
        Duration::ZERO
    }
}

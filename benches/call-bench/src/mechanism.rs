//! The mechanism alone: what one access costs through ordinary async and
//! through `nitori_call` when there is no work behind it. The resource only
//! bumps a counter. One task runs on a current-thread runtime, so locks are
//! never contended; the lock rows include the benchmark's own counting of
//! acquisitions, the same for every way.
//!
//! Each call makes 1 or 16 accesses, so the cost of starting and finishing a
//! call shows against the cost of an access. A release point after every
//! access shows what releasing costs: nothing to do with an exclusive
//! receiver, a release and a new acquisition with a lock.
//!
//! A step that cannot wait reads as a closure on a lent access. The rows of
//! that group compare it with `poll_fn` and `poll_access`, the way to write
//! the same step on a plain access, and each call is polled directly with a
//! view pinned by the caller, as a driver lending every view does. The rows
//! with two receivers use both in one step, nested on a plain access.
use crate::{
    calls::{Locked, Release, Shared},
    common::{Lock, LockKind, LockStats, PerLock, PollLock, Progress, with_lock},
};
use nitori_call::{
    BorrowedReceiver, Call, CallPoll, Lend, LentAccess, LentAccesses, LentCall, Receiver,
    ReceiverAccess, ReceiverFamily, ViewLoan, call, capability, family_receiver,
};
use std::{
    future::poll_fn,
    hint::black_box,
    pin::{Pin, pin},
    task::{Context, Poll, ready},
    time::{Duration, Instant},
};
use tokio::runtime::Runtime;

/// Accesses per measurement.
const ACCESSES: usize = 1 << 20;
const PER_CALL: [usize; 2] = [1, 16];
const REPEATS: usize = 9;

#[derive(Default)]
pub struct Counter {
    value: u64,
}
impl Counter {
    fn step(&mut self) -> u64 {
        self.value += 1;
        self.value
    }
}
impl Progress for Counter {
    fn progress(&self) -> u64 {
        self.value
    }
}

/// Bump the counter and return its new value.
#[capability]
pub trait Step: ReceiverFamily {
    fn poll_step<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
    ) -> Poll<u64>
    where
        Self: 'visit;
    /// The same step, which cannot wait.
    fn advance<'visit>(host: Pin<&mut Self::ReceiverView<'visit>>) -> u64
    where
        Self: 'visit;
}
family_receiver!(impl [] for Counter);
impl Step for Counter {
    fn poll_step<'visit>(
        host: Pin<&mut BorrowedReceiver<'visit, Counter>>,
        _: &mut Context<'_>,
    ) -> Poll<u64>
    where
        Self: 'visit,
    {
        Poll::Ready(host.get_mut().0.as_mut().get_mut().step())
    }
    #[inline]
    fn advance<'visit>(host: Pin<&mut BorrowedReceiver<'visit, Counter>>) -> u64
    where
        Self: 'visit,
    {
        host.get_mut().0.as_mut().get_mut().step()
    }
}
impl<L: Lock<Value = Counter>> Step for Locked<L> {
    fn poll_step<'visit>(
        host: Pin<&mut <Self as ReceiverFamily>::ReceiverView<'visit>>,
        _: &mut Context<'_>,
    ) -> Poll<u64>
    where
        Self: 'visit,
    {
        Poll::Ready(host.get_mut().0.step())
    }
    fn advance<'visit>(host: Pin<&mut <Self as ReceiverFamily>::ReceiverView<'visit>>) -> u64
    where
        Self: 'visit,
    {
        host.get_mut().0.step()
    }
}

/// Make `count` accesses, with a release point after each one when
/// `release`; return the last value.
#[call]
async fn steps(io: &mut impl ReceiverAccess<Family: Step>, count: usize, release: bool) -> u64 {
    let mut last = 0;
    for _ in 0..count {
        last = io.step().await;
        if release {
            io.release_view().await;
        }
    }
    last
}

/// Make `count` synchronous steps on a plain access.
#[call]
async fn polled_steps<F: Step>(io: &mut impl ReceiverAccess<Family = F>, count: usize) -> u64 {
    let mut last = 0;
    for _ in 0..count {
        last = poll_fn(|cx| {
            io.poll_access(cx, |access, _| Poll::Ready(F::advance(access.into_pin())))
        })
        .await;
    }
    last
}

/// Make `count` steps with `with` on a lent access.
#[call]
async fn lent_steps<F: Step>(io: &mut impl LentAccess<Family = F>, count: usize) -> u64 {
    let mut last = 0;
    for _ in 0..count {
        last = io.with(|access| F::advance(access.into_pin()));
    }
    last
}

/// Make `count` steps with the generated method on a lent access.
#[call]
async fn lent_method_steps(io: &mut impl LentAccess<Family: Step>, count: usize) -> u64 {
    let mut last = 0;
    for _ in 0..count {
        last = io.advance();
    }
    last
}

/// Step both receivers `count` times, nesting one access in the other.
#[call]
async fn polled_pairs<F: Step, G: Step>(
    first: &mut impl ReceiverAccess<Family = F>,
    second: &mut impl ReceiverAccess<Family = G>,
    count: usize,
) -> u64 {
    let mut last = 0;
    for _ in 0..count {
        last = poll_fn(|cx| {
            first.poll_access(cx, |first, cx| {
                second.poll_access(cx, |second, _| {
                    G::advance(second.into_pin());
                    Poll::Ready(F::advance(first.into_pin()))
                })
            })
        })
        .await;
    }
    last
}

/// Step both receivers `count` times in one synchronous step each.
#[call]
async fn lent_pairs<F: Step, G: Step>(
    first: &mut impl LentAccess<Family = F>,
    second: &mut impl LentAccess<Family = G>,
    count: usize,
) -> u64 {
    let mut last = 0;
    for _ in 0..count {
        last = (&mut *first, &mut *second).with(|(first, second)| {
            G::advance(second.into_pin());
            F::advance(first.into_pin())
        });
    }
    last
}

/// What ordinary async steps through.
trait PollStep {
    fn poll_step(&mut self, cx: &mut Context<'_>) -> Poll<u64>;
}
impl PollStep for Counter {
    fn poll_step(&mut self, _: &mut Context<'_>) -> Poll<u64> {
        Poll::Ready(self.step())
    }
}
/// A counter behind a lock, locked for each access.
struct LockEach<'a, L: Lock>(PollLock<'a, L>);
impl<L: Lock<Value = Counter>> PollStep for LockEach<'_, L> {
    fn poll_step(&mut self, cx: &mut Context<'_>) -> Poll<u64> {
        Poll::Ready(ready!(self.0.poll_lock(cx)).step())
    }
}

async fn plain_steps(source: &mut impl PollStep, count: usize) -> u64 {
    let mut last = 0;
    for _ in 0..count {
        last = poll_fn(|cx| source.poll_step(cx)).await;
    }
    last
}

/// Nanoseconds per access with `per_call` accesses per call, the median of
/// [`REPEATS`] after a warm-up. `run` makes all [`ACCESSES`] and returns the
/// counter's last value.
fn per_access(runtime: &Runtime, mut run: impl AsyncFnMut(usize) -> u64) -> [f64; 2] {
    PER_CALL.map(|per_call| {
        let mut measure = || {
            let start = Instant::now();
            let last = runtime.block_on(run(per_call));
            let elapsed = start.elapsed();
            assert_eq!(last, ACCESSES as u64);
            elapsed
        };
        measure();
        let mut times: Vec<Duration> = (0..REPEATS).map(|_| measure()).collect();
        times.sort();
        times[REPEATS / 2].as_nanos() as f64 / ACCESSES as f64
    })
}

fn row(name: &str, costs: [f64; 2]) {
    println!("  {name:<28} {:>11.1} {:>12.1}", costs[0], costs[1]);
}

/// The rows for a counter behind a lock.
struct RunLocked<'r> {
    runtime: &'r Runtime,
}
impl PerLock<Counter> for RunLocked<'_> {
    type Output = ();
    fn run<L: Lock<Value = Counter>>(self) {
        let runtime = self.runtime;
        row(
            "PlainLockPerLeaf",
            per_access(runtime, async |per_call| {
                let (lock, stats) = (L::new(Counter::default()), LockStats::new(false));
                let mut source = LockEach(PollLock::new(&lock, &stats));
                let mut last = 0;
                for _ in 0..ACCESSES / per_call {
                    last = plain_steps(&mut source, per_call).await;
                }
                black_box(last)
            }),
        );
        for (name, release, points) in [
            ("Call", Release::Never, false),
            ("Call, release points kept", Release::Never, true),
            ("CallRelease", Release::Always, true),
        ] {
            row(
                name,
                per_access(runtime, async |per_call| {
                    let (lock, stats) = (L::new(Counter::default()), LockStats::new(false));
                    let mut executor = Shared {
                        lock: &lock,
                        stats: &stats,
                        release,
                    };
                    let mut last = 0;
                    for _ in 0..ACCESSES / per_call {
                        last = executor.steps_unpin(per_call, points).await;
                    }
                    black_box(last)
                }),
            );
        }
    }
}

pub fn run(runtime: &Runtime) {
    println!(
        "mechanism: {ACCESSES} accesses to a counter, one task on current_thread, median of {REPEATS}; ns per access"
    );
    println!("  {:<28} {:>11} {:>12}", "", "1 per call", "16 per call");
    println!("exclusive");
    row(
        "Direct (no future)",
        per_access(runtime, async |per_call| {
            let mut counter = Counter::default();
            let mut last = 0;
            for _ in 0..ACCESSES / per_call {
                for _ in 0..per_call {
                    last = black_box(&mut counter).step();
                }
            }
            last
        }),
    );
    row(
        "Plain",
        per_access(runtime, async |per_call| {
            let mut counter = Counter::default();
            let mut last = 0;
            for _ in 0..ACCESSES / per_call {
                last = plain_steps(black_box(&mut counter), per_call).await;
            }
            last
        }),
    );
    for (name, points) in [("Call", false), ("Call, release points", true)] {
        row(
            name,
            per_access(runtime, async |per_call| {
                let mut counter = Counter::default();
                let mut last = 0;
                for _ in 0..ACCESSES / per_call {
                    last = black_box(&mut counter).steps_unpin(per_call, points).await;
                }
                last
            }),
        );
    }
    println!("synchronous steps, exclusive");
    row(
        "poll_fn + poll_access",
        per_access(runtime, async |per_call| {
            let mut counter = Counter::default();
            let mut last = 0;
            for _ in 0..ACCESSES / per_call {
                let mut call = pin!(Call::<Counter, _>::new(PolledSteps::new(per_call)));
                let mut view = pin!(Pin::new(black_box(&mut counter)).view());
                last = poll_fn(|cx| {
                    let lend = Some(Lend::Fixed(view.as_mut()));
                    match call.as_mut().poll_return_with(lend, cx) {
                        CallPoll::Ready(value) => Poll::Ready(value),
                        _ => Poll::Pending,
                    }
                })
                .await;
            }
            last
        }),
    );
    row(
        "LentCall, with",
        per_access(runtime, async |per_call| {
            let mut counter = Counter::default();
            let mut last = 0;
            for _ in 0..ACCESSES / per_call {
                let mut call = pin!(LentCall::<Counter, _>::new(LentSteps::new(per_call)));
                let mut view = pin!(Pin::new(black_box(&mut counter)).view());
                last = poll_fn(|cx| call.as_mut().poll_return_with_views(view.as_mut(), cx)).await;
            }
            last
        }),
    );
    row(
        "LentCall, method",
        per_access(runtime, async |per_call| {
            let mut counter = Counter::default();
            let mut last = 0;
            for _ in 0..ACCESSES / per_call {
                let mut call = pin!(LentCall::<Counter, _>::new(LentMethodSteps::new(per_call)));
                let mut view = pin!(Pin::new(black_box(&mut counter)).view());
                last = poll_fn(|cx| call.as_mut().poll_return_with_views(view.as_mut(), cx)).await;
            }
            last
        }),
    );
    row(
        "two, nested poll_access",
        per_access(runtime, async |per_call| {
            let (mut first, mut second) = (Counter::default(), Counter::default());
            let mut last = 0;
            for _ in 0..ACCESSES / per_call {
                let mut call = pin!(Call::<(Counter, Counter), _>::new(PolledPairs::new(
                    per_call
                )));
                let mut first = pin!(Pin::new(black_box(&mut first)).view());
                let mut second = pin!(Pin::new(black_box(&mut second)).view());
                last = poll_fn(|cx| {
                    let lends = (
                        Some(Lend::Fixed(ViewLoan::new(first.as_mut()))),
                        Some(Lend::Fixed(ViewLoan::new(second.as_mut()))),
                    );
                    match call.as_mut().poll_return_with_views(lends, cx) {
                        CallPoll::Ready(value) => Poll::Ready(value),
                        _ => Poll::Pending,
                    }
                })
                .await;
            }
            last
        }),
    );
    row(
        "two, tuple with",
        per_access(runtime, async |per_call| {
            let (mut first, mut second) = (Counter::default(), Counter::default());
            let mut last = 0;
            for _ in 0..ACCESSES / per_call {
                let mut call = pin!(LentCall::<(Counter, Counter), _>::new(LentPairs::new(
                    per_call
                )));
                let mut first = pin!(Pin::new(black_box(&mut first)).view());
                let mut second = pin!(Pin::new(black_box(&mut second)).view());
                last = poll_fn(|cx| {
                    call.as_mut()
                        .poll_return_with_views((first.as_mut(), second.as_mut()), cx)
                })
                .await;
            }
            last
        }),
    );
    for lock in [LockKind::Fair, LockKind::Unfair, LockKind::Blocking] {
        println!("{}", crate::lock_name(lock));
        with_lock(lock, RunLocked { runtime });
    }
}

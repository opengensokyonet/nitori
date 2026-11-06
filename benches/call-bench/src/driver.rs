//! Many calls over one state, the way a connection driver runs them: the
//! driver owns the state and lends it to each call it polls, so the calls
//! share it without a runtime borrow check.
//!
//! The state is a ring of `calls` slots holding tokens. Each call waits until
//! its slot has a token, then passes it on to the previous slot and wakes
//! that slot's call: one hop. Passing backwards is the worst case for a
//! driver that polls every call in order, which then polls every call once
//! for each hop of a token. `tokens` tokens circulate, one or one per call, so
//! that either one call or every call is ready at a time. A hop is the same
//! work in every way: two visits to the state, the second of which finds the
//! slot empty and registers the waker.
//!
//! Ways:
//! - `LentCall`: the driver owns the ring and lends it to each call;
//! - `Plain`: ordinary async futures sharing the ring as `Rc<RefCell<_>>`,
//!   borrowed for each visit;
//!
//! each polled by one of two drivers:
//! - woken: each call has a waker of its own that sets its bit in a bit set;
//!   a round polls the calls whose bits it takes;
//! - every: a round polls every call;
//!
//! and `FuturesUnordered` of the ordinary futures, which keeps its own queue
//! of woken futures. Everything runs on the calling thread, without a
//! runtime.
use futures_util::stream::{FuturesUnordered, StreamExt as _};
use nitori_call::{
    BorrowedReceiver, LentAccess, LentCall, Receiver, call, capability, family_receiver,
};
use std::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    hint::black_box,
    pin::Pin,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

/// Hops per measurement.
const HOPS: u64 = 1 << 20;
const REPEATS: usize = 9;

pub struct Ring {
    tokens: Vec<u32>,
    wakers: Vec<Option<Waker>>,
    /// Hops made, also read by the drivers between polls.
    hops: Rc<Cell<u64>>,
}
impl Ring {
    fn new(calls: usize, tokens: usize) -> Self {
        let mut ring = Ring {
            tokens: vec![0; calls],
            wakers: (0..calls).map(|_| None).collect(),
            hops: Rc::new(Cell::new(0)),
        };
        // Spread the tokens evenly.
        for token in 0..tokens {
            ring.tokens[token * calls / tokens] += 1;
        }
        ring
    }
    /// Pass a token from slot `index` to the previous one, or register to be woken
    /// when the slot gets one. `Ready(false)` once all hops are made.
    fn hop(&mut self, index: usize, cx: &mut Context<'_>) -> Poll<bool> {
        if self.hops.get() >= HOPS {
            return Poll::Ready(false);
        }
        if self.tokens[index] == 0 {
            match &mut self.wakers[index] {
                Some(waker) if waker.will_wake(cx.waker()) => {}
                slot => *slot = Some(cx.waker().clone()),
            }
            return Poll::Pending;
        }
        self.tokens[index] -= 1;
        // Backwards, against the order in which a round polls the calls: a
        // round that polls every call makes one hop per token, not a run of
        // them.
        let calls = self.tokens.len();
        let next = (index + calls - 1) % calls;
        self.tokens[next] += 1;
        self.hops.set(self.hops.get() + 1);
        if let Some(waker) = self.wakers[next].take() {
            waker.wake();
        }
        Poll::Ready(true)
    }
}

/// Pass tokens around a ring.
#[capability]
pub trait Hop: nitori_call::ReceiverFamily {
    fn hop<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        index: usize,
        cx: &mut Context<'_>,
    ) -> Poll<bool>
    where
        Self: 'visit;
}
family_receiver!(impl [] for Ring);
impl Hop for Ring {
    #[inline]
    fn hop<'visit>(
        host: Pin<&mut BorrowedReceiver<'visit, Ring>>,
        index: usize,
        cx: &mut Context<'_>,
    ) -> Poll<bool>
    where
        Self: 'visit,
    {
        host.get_pin_mut().get_mut().hop(index, cx)
    }
}

/// Make hops from slot `index` until all are made.
#[call]
async fn relay<F: Hop>(ring: &mut impl LentAccess<Family = F>, index: usize) {
    while poll_fn(|cx| ring.with(|ring| F::hop(ring.into_pin(), index, cx))).await {}
}

/// The same with ordinary async.
async fn plain_relay(ring: Rc<RefCell<Ring>>, index: usize) {
    while poll_fn(|cx| ring.borrow_mut().hop(index, cx)).await {}
}

/// The wakers of the woken driver: waking sets the call's bit.
struct Marks(Vec<AtomicU64>);
struct Mark {
    marks: Arc<Marks>,
    index: usize,
}
impl Wake for Mark {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.marks.0[self.index / 64].fetch_or(1 << (self.index % 64), Ordering::Release);
    }
}
fn mark_wakers(calls: usize) -> (Arc<Marks>, Vec<Waker>) {
    let marks = Arc::new(Marks(
        (0..calls.div_ceil(64)).map(|_| AtomicU64::new(0)).collect(),
    ));
    // Every call is polled once at the start.
    for (word, bits) in marks.0.iter().enumerate() {
        let count = (calls - word * 64).min(64);
        bits.store(
            if count == 64 {
                u64::MAX
            } else {
                (1 << count) - 1
            },
            Ordering::Relaxed,
        );
    }
    let wakers = (0..calls)
        .map(|index| {
            Waker::from(Arc::new(Mark {
                marks: marks.clone(),
                index,
            }))
        })
        .collect();
    (marks, wakers)
}
/// Poll `poll(index, cx)` for every call whose bit is set, a round at a
/// time, until `done()`.
fn run_woken(calls: usize, done: impl Fn() -> bool, mut poll: impl FnMut(usize, &mut Context<'_>)) {
    let (marks, wakers) = mark_wakers(calls);
    while !done() {
        for (word, bits) in marks.0.iter().enumerate() {
            let mut taken = bits.swap(0, Ordering::Acquire);
            while taken != 0 {
                let index = word * 64 + taken.trailing_zeros() as usize;
                taken &= taken - 1;
                poll(index, &mut Context::from_waker(&wakers[index]));
            }
        }
    }
}
/// Poll every call each round until `done()`.
fn run_every(calls: usize, done: impl Fn() -> bool, mut poll: impl FnMut(usize, &mut Context<'_>)) {
    let mut cx = Context::from_waker(Waker::noop());
    while !done() {
        for index in 0..calls {
            poll(index, &mut cx);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Way {
    LentWoken,
    LentEvery,
    PlainWoken,
    PlainEvery,
    FuturesUnordered,
}
const WAYS: [Way; 5] = [
    Way::LentWoken,
    Way::LentEvery,
    Way::PlainWoken,
    Way::PlainEvery,
    Way::FuturesUnordered,
];

fn run_once(way: Way, calls: usize, tokens: usize) -> Duration {
    let mut ring = Ring::new(calls, tokens);
    let hops = ring.hops.clone();
    let done = || hops.get() >= HOPS;
    let start;
    match way {
        Way::LentWoken | Way::LentEvery => {
            let mut relays: Vec<_> = (0..calls)
                .map(|index| Box::pin(LentCall::<Ring, _>::new(Relay::new(index))))
                .collect();
            start = Instant::now();
            let poll = |index: usize, cx: &mut Context<'_>| {
                let mut view = Pin::new(&mut ring).view();
                let _ = relays[index]
                    .as_mut()
                    .poll_return_with_views(Pin::new(&mut view), cx);
            };
            match way {
                Way::LentWoken => run_woken(calls, done, poll),
                _ => run_every(calls, done, poll),
            }
        }
        Way::PlainWoken | Way::PlainEvery | Way::FuturesUnordered => {
            let ring = Rc::new(RefCell::new(ring));
            match way {
                Way::FuturesUnordered => {
                    let mut relays: FuturesUnordered<_> = (0..calls)
                        .map(|index| plain_relay(ring.clone(), index))
                        .collect();
                    start = Instant::now();
                    let mut cx = Context::from_waker(Waker::noop());
                    while !done() {
                        let _ = relays.poll_next_unpin(&mut cx);
                    }
                }
                _ => {
                    let mut relays: Vec<_> = (0..calls)
                        .map(|index| Box::pin(plain_relay(ring.clone(), index)))
                        .collect();
                    start = Instant::now();
                    let poll = |index: usize, cx: &mut Context<'_>| {
                        let _ = relays[index].as_mut().poll(cx);
                    };
                    match way {
                        Way::PlainWoken => run_woken(calls, done, poll),
                        _ => run_every(calls, done, poll),
                    }
                }
            }
        }
    }
    let elapsed = start.elapsed();
    assert_eq!(black_box(hops.get()), HOPS, "{way:?}");
    elapsed
}

pub fn run() {
    println!(
        "driver: {HOPS} hops around a ring of calls, on the calling thread, median of {REPEATS} (min–max); ns per hop"
    );
    println!(
        "sizes: LentCall {} bytes, ordinary future {} bytes",
        size_of::<LentCall<Ring, Relay>>(),
        size_of_val(&plain_relay(Rc::new(RefCell::new(Ring::new(1, 1))), 0)),
    );
    for calls in [4, 64, 1024] {
        for tokens in [1, calls] {
            println!("{calls} calls, {tokens} tokens");
            for way in WAYS {
                run_once(way, calls, tokens);
                let mut times: Vec<Duration> =
                    (0..REPEATS).map(|_| run_once(way, calls, tokens)).collect();
                times.sort();
                let per_hop = |time: Duration| time.as_nanos() as f64 / HOPS as f64;
                println!(
                    "  {:<20} {:8.1} ({:6.1}–{:6.1})",
                    format!("{way:?}"),
                    per_hop(times[REPEATS / 2]),
                    per_hop(times[0]),
                    per_hop(times[REPEATS - 1]),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_way_makes_every_hop() {
        for (calls, tokens) in [(1, 1), (3, 1), (70, 70), (70, 5)] {
            for way in WAYS {
                // `run_once` checks the count.
                run_once(way, calls, tokens);
            }
        }
    }
}

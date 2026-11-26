//! The driver run on a small structure of its own, and on the structures
//! of the lending's checks.
extern crate std;

use alloc::sync::Arc;
use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use std::task::Wake;

use super::{Core, Driver, Phase};
use crate::call::Composition;

mod args;
mod deep;
mod edit;
mod ending;
mod fixture;
mod grouped_build;
mod numbers;
mod operate;
mod panics;
mod priority;
mod react;
mod requests;
mod wakers;

/// The live core of `d`.
fn core<Q: Composition>(d: &mut Driver<Q>) -> &mut Core<Q> {
    match &mut d.phase {
        Phase::Live(core) => core,
        Phase::Ended | Phase::Panicked => panic!("the driver is not live"),
    }
}

/// An outer waker that counts its wakes.
#[derive(Default)]
struct Counting(AtomicUsize);
impl Wake for Counting {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
/// A counting outer waker, and the waker.
fn outer() -> (Arc<Counting>, Waker) {
    let c = Arc::new(Counting::default());
    (c.clone(), Waker::from(c))
}
/// Poll `d` once with `w`.
fn poll<Q: Composition>(d: &mut Driver<Q>, w: &Waker) -> Poll<()> {
    Pin::new(d).poll(&mut Context::from_waker(w))
}

#[cfg(feature = "std")]
#[test]
fn the_driver_is_send() {
    fn send<T: Send>() {}
    send::<Driver<fixture::QT>>();
    send::<Driver<crate::call::tests::Q1>>();
}

//! The entries of the timer table: clearing the earliest, nodes that
//! end, a new outer waker, setting again in a wake, and `get`.
#![feature(impl_trait_in_assoc_type)]
mod common;

use nitori_driver::composition;

pub struct Q;
composition!(impl for Q: common::Top, nitori_driver::frunk_core::HList![common::Run, nitori_driver_timer::Part<Q>], common::FakeClock, common::Level);
use std::{future::Future, pin::Pin, task::Context, time::Duration};

use nitori_driver::{
    driver::Driver,
    frunk_core::{HList, hlist, hlist_pat},
    need::{Mut, Requests, WakerOf},
    request::NodeRequest,
    structure::KeyAddress,
};
use nitori_driver_timer::TimerTable;

use common::*;

type Table = TimerTable<FakeClock, Level>;
const fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// A driver with the leaves `keys`, polled until it waits.
fn driver(clock: &FakeClock, keys: &[u32]) -> (Driver<Q>, std::task::Waker) {
    let mut d = Driver::<Q>::new(clock.clone(), (), hlist![(), ()], Level::Low);
    for &k in keys {
        d.operate::<Top, HList![Requests<NodeRequest<Top, Top>>], _, _, _>(
            &KeyAddress::here(),
            |hlist_pat![r]| r.push(NodeRequest::new(KeyAddress::here(), k)),
        )
        .unwrap()
        .unwrap();
    }
    let (_, w) = outer();
    poll(&mut d, &w);
    woken();
    clock.made();
    (d, w)
}
fn poll(d: &mut Driver<Q>, w: &std::task::Waker) {
    let _ = Pin::new(d).poll(&mut Context::from_waker(w));
}
fn set(d: &mut Driver<Q>, leaf: u32, at: Duration, p: Level) {
    d.operate::<Leaf, HList![Mut<Table>, WakerOf<Alarm>], _, _, _>(
        &KeyAddress::<Top, Top>::child::<Leaf, _>(leaf),
        |hlist_pat![t, w]| t.set(at, &w, p),
    )
    .unwrap()
    .unwrap();
}
fn clear(d: &mut Driver<Q>, leaf: u32) {
    d.operate::<Leaf, HList![Mut<Table>, WakerOf<Alarm>], _, _, _>(
        &KeyAddress::<Top, Top>::child::<Leaf, _>(leaf),
        |hlist_pat![t, w]| t.clear(&w),
    )
    .unwrap()
    .unwrap();
}

use nitori_driver::need::ComponentWaker;
use std::{sync::atomic::Ordering::SeqCst, task::Waker};

fn waker_of(d: &mut Driver<Q>, leaf: u32) -> ComponentWaker {
    d.operate::<Leaf, HList![WakerOf<Alarm>], _, _, _>(
        &KeyAddress::<Top, Top>::child::<Leaf, _>(leaf),
        |hlist_pat![w]| w,
    )
    .unwrap()
    .unwrap()
}
fn get(d: &mut Driver<Q>, w: ComponentWaker) -> Option<Duration> {
    d.operate::<Top, HList![Mut<Table>], _, _, _>(&KeyAddress::here(), |hlist_pat![t]| t.get(&w))
        .unwrap()
        .unwrap()
}
fn request(d: &mut Driver<Q>, k: u32) {
    d.operate::<Top, HList![Requests<NodeRequest<Top, Top>>], _, _, _>(
        &KeyAddress::here(),
        |hlist_pat![r]| r.push(NodeRequest::new(KeyAddress::here(), k)),
    )
    .unwrap()
    .unwrap();
}
fn no_instants() -> [Duration; 0] {
    []
}

#[test]
fn clearing_the_earliest_entry_wakes_nothing_and_the_call_looks_again_at_its_instant() {
    let clock = FakeClock::default();
    let (mut d, w) = driver(&clock, &[1, 2]);
    set(&mut d, 1, s(5), Level::Low);
    set(&mut d, 2, s(8), Level::Low);
    poll(&mut d, &w);
    assert_eq!(clock.made(), [s(5)]);
    clear(&mut d, 1);
    poll(&mut d, &w);
    assert_eq!(clock.made(), no_instants());
    clock.advance(s(5));
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
    assert_eq!(clock.made(), [s(8)]);
    clock.advance(s(8));
    poll(&mut d, &w);
    assert_eq!(woken(), [2]);
}

#[test]
fn an_alarm_that_clears_in_its_end_leaves_no_entry_and_a_new_leaf_of_the_key_sets_afresh() {
    let clock = FakeClock::default();
    clear_on_end(true);
    let (mut d, w) = driver(&clock, &[1, 2]);
    let old = waker_of(&mut d, 1);
    set(&mut d, 1, s(50), Level::High);
    set(&mut d, 2, s(60), Level::Low);
    poll(&mut d, &w);
    assert_eq!(get(&mut d, old), Some(s(50)));
    request(&mut d, 1001);
    poll(&mut d, &w);
    assert_eq!(get(&mut d, old), None);
    request(&mut d, 1);
    poll(&mut d, &w);
    set(&mut d, 1, s(70), Level::Low);
    poll(&mut d, &w);
    clock.advance(s(50));
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
    clock.advance(s(60));
    poll(&mut d, &w);
    assert_eq!(woken(), [2]);
    clock.advance(s(70));
    poll(&mut d, &w);
    assert_eq!(woken(), [1]);
}

#[test]
fn an_alarm_that_does_not_clear_leaves_its_entry_to_its_instant_and_nothing_is_woken() {
    let clock = FakeClock::default();
    clear_on_end(false);
    let (mut d, w) = driver(&clock, &[1, 2]);
    let old = waker_of(&mut d, 1);
    set(&mut d, 1, s(50), Level::High);
    set(&mut d, 2, s(60), Level::Low);
    poll(&mut d, &w);
    request(&mut d, 1001);
    poll(&mut d, &w);
    assert_eq!(get(&mut d, old), Some(s(50)));
    clock.made();
    clock.advance(s(50));
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
    assert_eq!(get(&mut d, old), None);
    assert_eq!(clock.made(), [s(60)]);
    clock.advance(s(60));
    poll(&mut d, &w);
    assert_eq!(woken(), [2]);
}

#[test]
fn a_set_earlier_after_the_outer_waker_changed_wakes_the_new_outer_waker() {
    let clock = FakeClock::default();
    let (mut d, w1) = driver(&clock, &[1, 2]);
    set(&mut d, 1, s(10), Level::Low);
    poll(&mut d, &w1);
    let (c2, w2): (_, Waker) = outer();
    poll(&mut d, &w2);
    clock.made();
    let before2 = c2.0.load(SeqCst);
    set(&mut d, 2, s(5), Level::Low);
    assert_eq!(c2.0.load(SeqCst), before2 + 1);
    poll(&mut d, &w2);
    assert_eq!(clock.made(), [s(5)]);
    clock.advance(s(5));
    assert_eq!(c2.0.load(SeqCst), before2 + 2);
    poll(&mut d, &w2);
    assert_eq!(woken(), [2]);
}

#[test]
fn an_alarm_that_sets_its_next_instant_in_its_wake_is_woken_once_per_instant() {
    let clock = FakeClock::default();
    let (mut d, w) = driver(&clock, &[1]);
    next_instants(&[s(6), s(9)]);
    set(&mut d, 1, s(3), Level::Low);
    poll(&mut d, &w);
    assert_eq!(clock.made(), [s(3)]);
    clock.advance(s(3));
    poll(&mut d, &w);
    assert_eq!(woken(), [1]);
    assert_eq!(clock.made(), [s(6)]);
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
    assert_eq!(clock.made(), no_instants());
    clock.advance(s(6));
    poll(&mut d, &w);
    assert_eq!(woken(), [1]);
    assert_eq!(clock.made(), [s(9)]);
    clock.advance(s(9));
    poll(&mut d, &w);
    assert_eq!(woken(), [1]);
    assert_eq!(clock.made(), no_instants());
    clock.advance(s(30));
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
    assert_eq!(clock.made(), no_instants());
}

#[test]
fn get_is_none_before_a_set_the_instant_after_it_and_none_after_a_clear_or_a_wake() {
    let clock = FakeClock::default();
    let (mut d, w) = driver(&clock, &[1, 2]);
    let w1 = waker_of(&mut d, 1);
    let w2 = waker_of(&mut d, 2);
    assert_eq!(get(&mut d, w1), None);
    set(&mut d, 1, s(5), Level::Low);
    assert_eq!(get(&mut d, w1), Some(s(5)));
    assert_eq!(get(&mut d, w2), None);
    set(&mut d, 1, s(8), Level::Low);
    assert_eq!(get(&mut d, w1), Some(s(8)));
    clear(&mut d, 1);
    assert_eq!(get(&mut d, w1), None);
    set(&mut d, 2, s(4), Level::Low);
    poll(&mut d, &w);
    assert_eq!(get(&mut d, w2), Some(s(4)));
    clock.advance(s(4));
    poll(&mut d, &w);
    assert_eq!(woken(), [2]);
    assert_eq!(get(&mut d, w2), None);
}

//! The timer table and its part on a structure of leaves with alarms.
#![feature(impl_trait_in_assoc_type)]
mod common;

use nitori_driver::composition;

pub struct Q;
composition!(impl for Q: common::Top, nitori_driver::frunk_core::HList![common::Run, nitori_driver_timer::Part<Q>], common::FakeClock, common::Level);
use std::{future::Future, pin::Pin, task::Context, time::Duration};

use nitori_driver::{
    driver::Driver,
    frunk_core::{HList, hlist, hlist_pat},
    need::{Mut, Requests, WakerOf, Wakes},
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
fn wake(d: &mut Driver<Q>, leaf: u32, p: Level) {
    d.operate::<Leaf, HList![Wakes<Level>, WakerOf<Alarm>], _, _, _>(
        &KeyAddress::<Top, Top>::child::<Leaf, _>(leaf),
        |hlist_pat![wakes, w]| wakes.wake(&w, p),
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

#[test]
fn a_set_instant_wakes_its_component_at_its_priority() {
    let clock = FakeClock::default();
    let (mut d, w) = driver(&clock, &[1, 2, 3]);
    set(&mut d, 1, s(5), Level::Low);
    set(&mut d, 2, s(5), Level::High);
    set(&mut d, 3, s(5), Level::Mid);
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
    clock.advance(s(4));
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
    clock.advance(s(5));
    poll(&mut d, &w);
    assert_eq!(woken(), [2, 3, 1]);
    // once only
    clock.advance(s(9));
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
}

#[test]
fn a_set_earlier_rearms_the_timer() {
    let clock = FakeClock::default();
    let (mut d, w) = driver(&clock, &[1, 2]);
    set(&mut d, 1, s(10), Level::Low);
    poll(&mut d, &w);
    assert_eq!(clock.made(), [s(10)]);
    set(&mut d, 2, s(5), Level::Low);
    poll(&mut d, &w);
    assert_eq!(clock.made(), [s(5)]);
    // a later one does not
    set(&mut d, 2, s(7), Level::Low);
    poll(&mut d, &w);
    assert_eq!(clock.made(), [Duration::ZERO; 0]);
    clock.advance(s(5));
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
    assert_eq!(clock.made(), [s(7)]);
    clock.advance(s(7));
    poll(&mut d, &w);
    assert_eq!(woken(), [2]);
    assert_eq!(clock.made(), [s(10)]);
    clock.advance(s(10));
    poll(&mut d, &w);
    assert_eq!(woken(), [1]);
    assert_eq!(clock.made(), [Duration::ZERO; 0]);
}

#[test]
fn a_cleared_instant_wakes_nothing() {
    let clock = FakeClock::default();
    let (mut d, w) = driver(&clock, &[1, 2]);
    set(&mut d, 1, s(5), Level::Low);
    set(&mut d, 2, s(6), Level::Low);
    poll(&mut d, &w);
    clear(&mut d, 1);
    clock.advance(s(6));
    poll(&mut d, &w);
    assert_eq!(woken(), [2]);
}

#[test]
fn a_second_set_of_one_waker_replaces_the_first() {
    let clock = FakeClock::default();
    let (mut d, w) = driver(&clock, &[1]);
    set(&mut d, 1, s(5), Level::Low);
    set(&mut d, 1, s(8), Level::Low);
    poll(&mut d, &w);
    clock.advance(s(5));
    poll(&mut d, &w);
    assert_eq!(woken(), [0u32; 0]);
    clock.advance(s(8));
    poll(&mut d, &w);
    assert_eq!(woken(), [1]);
}

#[test]
fn a_set_already_due_wakes_on_the_next_poll() {
    let clock = FakeClock::default();
    clock.advance(s(3));
    let (mut d, w) = driver(&clock, &[1]);
    set(&mut d, 1, s(2), Level::High);
    poll(&mut d, &w);
    assert_eq!(woken(), [1]);
}

#[test]
fn a_timer_ended_before_the_reading_wakes_what_it_was_for() {
    let clock = FakeClock::default();
    let (mut d, w) = driver(&clock, &[1, 2]);
    set(&mut d, 1, s(5), Level::Low);
    set(&mut d, 2, s(8), Level::Low);
    poll(&mut d, &w);
    assert_eq!(clock.made(), [s(5)]);
    // the timer for 5 ends while the reading is still 0
    clock.end_early();
    poll(&mut d, &w);
    assert_eq!(woken(), [1]);
    assert_eq!(clock.made(), [s(8)]);
}

#[test]
fn the_call_is_woken_at_the_highest_priority_of_all_entries() {
    let clock = FakeClock::default();
    let (mut d, w) = driver(&clock, &[1, 2, 3, 4]);
    // the earliest is low, a later one high: the call sets high for itself
    set(&mut d, 1, s(5), Level::Low);
    set(&mut d, 2, s(10), Level::High);
    poll(&mut d, &w);
    // leaf 4 is woken at mid; leaf 3 is set already due at high, which
    // wakes the call at the priority it set
    wake(&mut d, 4, Level::Mid);
    set(&mut d, 3, s(0), Level::High);
    poll(&mut d, &w);
    assert_eq!(woken(), [3, 4]);
}

#[test]
fn a_driver_with_the_part_is_send() {
    fn send<T: Send>() {}
    send::<Driver<Q>>();
}

use super::*;
use std::{panic, sync::Mutex, thread};

fn increment(state: &impl SharedState<State = u32>) {
    state.with(|count| *count += 1);
}

fn read(state: &impl SharedState<State = u32>) -> u32 {
    state.with(|count| *count)
}

#[test]
fn every_wrapper_of_a_cell_reaches_the_same_state() {
    let mut cell = RefCell::new(0);
    increment(&cell);
    increment(&&cell);
    increment(&&mut cell);
    let mut exclusive = &mut cell;
    <&mut RefCell<u32> as SharedState>::with_mut(&mut exclusive, |count| *count += 1);
    assert_eq!(read(&cell), 4);
    let shared = Rc::new(cell);
    increment(&shared);
    assert_eq!(read(&shared), 5);
}

#[test]
#[should_panic(expected = "already")]
fn borrowing_a_cell_again_while_an_operation_runs_panics() {
    let cell = RefCell::new(0);
    cell.with(|_| cell.with(|_| ()));
}

#[test]
fn every_wrapper_of_a_mutex_reaches_the_same_state() {
    let mut mutex = Mutex::new(0);
    increment(&mutex);
    increment(&&mutex);
    mutex.with_mut(|count| *count += 1);
    let shared = Rc::new(mutex);
    increment(&shared);
    assert_eq!(read(&shared), 4);
    let shared = Arc::new(Mutex::new(0));
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| increment(&shared));
        }
    });
    assert_eq!(read(&shared), 4);
}

#[test]
fn a_poisoned_mutex_is_taken_as_it_is() {
    let mut mutex = Mutex::new(1);
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        mutex.with(|count| {
            *count = 2;
            panic!("poison it");
        })
    }));
    assert!(result.is_err());
    assert!(mutex.is_poisoned());
    assert_eq!(read(&mutex), 2);
    mutex.with_mut(|count| *count = 3);
    assert_eq!(read(&mutex), 3);
}

fn shared_count<Y: Synchronization>() -> u32 {
    let state = Y::share(0);
    let clone = state.clone();
    increment(&clone);
    increment(&state);
    read(&state)
}

#[test]
fn a_clone_of_a_family_s_wrapper_is_the_same_state() {
    assert_eq!(shared_count::<RefCellSynchronization>(), 2);
    assert_eq!(shared_count::<MutexSynchronization>(), 2);
}

#[test]
fn the_mutex_family_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<<MutexSynchronization as Synchronization>::SharedState<u32>>();
}

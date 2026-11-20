#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![warn(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

extern crate alloc;
#[cfg(all(feature = "std", not(test)))]
extern crate std;

use alloc::{rc::Rc, sync::Arc};
use core::{cell::RefCell, fmt};

/// State that its owners borrow `&mut` for one synchronous operation at a
/// time (`SharedState`).
///
/// An operation must not borrow the same state again while it runs.
pub trait SharedState {
    /// The state.
    type State: ?Sized;
    /// Runs `op` on the state, borrowed for as long as it runs.
    fn with<R>(&self, op: impl FnOnce(&mut Self::State) -> R) -> R;
    /// Runs `op` on the state through exclusive access, which needs no
    /// runtime check where the wrapper can tell it is exclusive.
    fn with_mut<R>(&mut self, op: impl FnOnce(&mut Self::State) -> R) -> R {
        self.with(op)
    }
}

/// State on one thread: the borrow is the cell's, and a borrow while
/// another runs panics.
impl<T: ?Sized> SharedState for RefCell<T> {
    type State = T;
    fn with<R>(&self, op: impl FnOnce(&mut T) -> R) -> R {
        op(&mut self.borrow_mut())
    }
    fn with_mut<R>(&mut self, op: impl FnOnce(&mut T) -> R) -> R {
        op(self.get_mut())
    }
}

/// State shared between threads: each operation holds the lock while it
/// runs. A lock a panic poisoned is taken as it is.
#[cfg(feature = "std")]
impl<T: ?Sized> SharedState for std::sync::Mutex<T> {
    type State = T;
    fn with<R>(&self, op: impl FnOnce(&mut T) -> R) -> R {
        op(&mut self
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }
    fn with_mut<R>(&mut self, op: impl FnOnce(&mut T) -> R) -> R {
        op(self
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

impl<S: SharedState + ?Sized> SharedState for &S {
    type State = S::State;
    fn with<R>(&self, op: impl FnOnce(&mut S::State) -> R) -> R {
        (**self).with(op)
    }
}

impl<S: SharedState + ?Sized> SharedState for &mut S {
    type State = S::State;
    fn with<R>(&self, op: impl FnOnce(&mut S::State) -> R) -> R {
        (**self).with(op)
    }
    fn with_mut<R>(&mut self, op: impl FnOnce(&mut S::State) -> R) -> R {
        (**self).with_mut(op)
    }
}

impl<S: SharedState + ?Sized> SharedState for Rc<S> {
    type State = S::State;
    fn with<R>(&self, op: impl FnOnce(&mut S::State) -> R) -> R {
        (**self).with(op)
    }
}

impl<S: SharedState + ?Sized> SharedState for Arc<S> {
    type State = S::State;
    fn with<R>(&self, op: impl FnOnce(&mut S::State) -> R) -> R {
        (**self).with(op)
    }
}

/// How state shared between the users of a library is synchronized: one
/// family of wrappers (`Synchronization`), which the host chooses once.
///
/// The library builds each wrapper with [`share`](Self::share) and clones it
/// for every user; a clone is the same state. Whether the library's values
/// are `Send` and `Sync` follows from the family.
pub trait Synchronization: 'static {
    /// The wrapper of state of type `T`.
    type SharedState<T>: SharedState<State = T> + Clone;
    /// `state`, in a new wrapper.
    fn share<T>(state: T) -> Self::SharedState<T>;
}

/// The family of state on one thread (`RefCellSynchronization`): each state
/// in a cell behind an `Rc`.
///
/// Nothing of it is `Send` or `Sync`.
pub enum RefCellSynchronization {}

impl fmt::Debug for RefCellSynchronization {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl Synchronization for RefCellSynchronization {
    type SharedState<T> = Rc<RefCell<T>>;
    fn share<T>(state: T) -> Rc<RefCell<T>> {
        Rc::new(RefCell::new(state))
    }
}

/// The family of state shared between threads (`MutexSynchronization`):
/// each state behind a mutex in an `Arc`.
///
/// Its wrappers are `Send` and `Sync` when the state is `Send`.
#[cfg(feature = "std")]
pub enum MutexSynchronization {}

#[cfg(feature = "std")]
impl fmt::Debug for MutexSynchronization {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

#[cfg(feature = "std")]
impl Synchronization for MutexSynchronization {
    type SharedState<T> = Arc<std::sync::Mutex<T>>;
    fn share<T>(state: T) -> Arc<std::sync::Mutex<T>> {
        Arc::new(std::sync::Mutex::new(state))
    }
}

#[cfg(test)]
mod tests;

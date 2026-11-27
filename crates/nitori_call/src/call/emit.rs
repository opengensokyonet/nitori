//! The future of one emission.
use super::{Emitter, ItemCell};
use crate::access::polled_directly;
use core::{
    future::Future,
    mem,
    pin::Pin,
    task::{Context, Poll},
};

/// Where an emission's item is.
enum State<Y> {
    /// Not polled yet: the emission has not begun.
    Unpolled(Y),
    /// Polled while the cell held another item of the same poll.
    Waiting(Y),
    /// In the cell, until the caller has taken this many items.
    Placed {
        delivered: usize,
    },
    Done,
}

/// Hands an item to the caller of the operation and completes once the
/// caller has taken it.
///
/// It completes only after the delivery, not on any later poll: an
/// intermediate executor such as `FuturesUnordered` polls a branch that woke
/// itself again in the same poll, before the caller has taken the item.
pub(super) struct Emit<'a, 's, Y> {
    emitter: &'a mut Emitter<'s, Y>,
    state: State<Y>,
}
// No field is pinned structurally.
impl<Y> Unpin for Emit<'_, '_, Y> {}

impl<'a, 's, Y> Emit<'a, 's, Y> {
    pub(super) fn new(emitter: &'a mut Emitter<'s, Y>, item: Y) -> Self {
        Self {
            emitter,
            state: State::Unpolled(item),
        }
    }
}

/// The cell `emitter` emits into, borrowed apart from the emitter.
///
/// # Safety
/// The cell must outlive `'c`, and an item of the emitter must exist.
unsafe fn cell<'c, Y>(emitter: &Emitter<'_, Y>) -> &'c ItemCell<Y> {
    // SAFETY: the caller guarantees the lifetime; only an emitter of
    // `!` items has no cell, and no item of it exists.
    unsafe { &*emitter.cell }
}

impl<Y> Future for Emit<'_, '_, Y> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        // SAFETY: the owner of the cell outlives its operation, which owns
        // this emission; the emission has had an item since it was made.
        let cell = unsafe { cell(this.emitter) };
        match mem::replace(&mut this.state, State::Done) {
            State::Done => return Poll::Ready(()),
            State::Placed { delivered } => {
                if cell.delivered.get() != delivered {
                    return Poll::Ready(());
                }
                this.state = State::Placed { delivered };
            }
            State::Unpolled(item) | State::Waiting(item) => {
                if cell.discard.get() {
                    return Poll::Ready(());
                }
                match cell.item.take() {
                    // An earlier emission in this poll is not yet delivered.
                    Some(earlier) => {
                        cell.item.set(Some(earlier));
                        this.state = State::Waiting(item);
                    }
                    None => {
                        cell.item.set(Some(item));
                        this.state = State::Placed {
                            delivered: cell.delivered.get(),
                        };
                    }
                }
            }
        }
        // Intermediate executors only repoll branches whose waker fired.
        // With none, the item itself makes the poll ready.
        if !polled_directly(cell.root.get(), cx) {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }
}

impl<Y> Drop for Emit<'_, '_, Y> {
    /// An emission dropped after its first poll still delivers its item. One
    /// that is waiting puts it in the cell, if no other item of this poll is
    /// there; if one still is, the item is lost, as `Emitter::emit` says.
    fn drop(&mut self) {
        let State::Waiting(item) = mem::replace(&mut self.state, State::Done) else {
            return;
        };
        // SAFETY: as in `poll`.
        let cell = unsafe { cell(self.emitter) };
        if cell.discard.get() {
            return;
        }
        match cell.item.take() {
            Some(earlier) => cell.item.set(Some(earlier)),
            None => cell.item.set(Some(item)),
        }
    }
}

//! The storage a [`WriteLent`](crate::WriteLent) sink lends for one fill,
//! what the fill produced, and what the loan came to.
use crate::{
    LentSegment,
    error::{self, InsufficientCapacityError},
};
use bytes::{BufMut, buf::Limit};
use core::{fmt, num::NonZeroUsize};

/// Storage a sink lends to a fill: room for at most [`granted`] bytes.
///
/// The fill puts bytes through [`buffer`], a [`Limit`] at the granted
/// count, for example by passing it to
/// [`Read::poll_read`](crate::Read::poll_read). The sink keeps the buffer:
/// when the fill returns, the sink counts what it keeps from its own storage,
/// not from what the fill reports.
///
/// [`granted`]: LentBuffer::granted
/// [`buffer`]: LentBuffer::buffer
pub struct LentBuffer<B> {
    buffer: Limit<B>,
    granted: NonZeroUsize,
    /// The storage's `remaining_mut` when it was lent.
    room: usize,
}

impl<B: BufMut> LentBuffer<B> {
    /// Lend `storage` for at most `granted` bytes. A sink calls this in
    /// [`WriteLent::poll_write_lent`](crate::WriteLent::poll_write_lent)
    /// with storage that has room for all of them.
    ///
    /// # Errors
    ///
    /// Fails when `storage` has room for fewer than `granted` bytes.
    pub fn new(storage: B, granted: NonZeroUsize) -> Result<Self, InsufficientCapacityError> {
        let room = storage.remaining_mut();
        if room < granted.get() {
            return Err(error::InsufficientCapacitySnafu {
                required: granted.get(),
                available: room,
            }
            .build());
        }
        Ok(Self {
            buffer: storage.limit(granted.get()),
            granted,
            room,
        })
    }

    /// The most bytes the sink will keep from this buffer.
    #[must_use]
    pub fn granted(&self) -> NonZeroUsize {
        self.granted
    }

    /// The bytes put into the buffer so far: the decrease of the storage's
    /// `remaining_mut`, or zero when it grew.
    #[must_use]
    pub fn filled(&self) -> usize {
        self.room
            .saturating_sub(self.buffer.get_ref().remaining_mut())
    }

    /// The buffer to put bytes into, limited to what is still granted.
    ///
    /// Raising the limit with [`Limit::set_limit`], or changing the storage
    /// through [`Limit::get_mut`] other than by putting bytes, breaks the
    /// loan: the sink then keeps nothing and answers
    /// [`LentOutcome::NotKept`].
    pub fn buffer(&mut self) -> &mut Limit<B> {
        &mut self.buffer
    }

    /// The lent storage, for the sink to count what it keeps.
    pub fn into_storage(self) -> B {
        self.buffer.into_inner()
    }
}

impl<B: BufMut> fmt::Debug for LentBuffer<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LentBuffer")
            .field("granted", &self.granted)
            .field("filled", &self.filled())
            .finish_non_exhaustive()
    }
}

/// What one loan of [`WriteLent`](crate::WriteLent) came to, once the sink
/// lent and called the fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LentOutcome<R, T, A> {
    /// The sink kept the bytes the fill put, at least one.
    Kept(LentSegment<R, T>),
    /// The fill returned its value, but the sink kept nothing: no byte was
    /// put, or the loan was broken by putting more than the granted count or
    /// changing the storage otherwise.
    NotKept(T),
    /// The fill gave up, and nothing is written.
    GaveUp(A),
}

/// What a fill put into a lent buffer produced: its value and the write
/// options of the bytes it put, which the sink writes them with when it
/// keeps them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Filled<T, O> {
    /// The value the sink hands back in [`LentOutcome`].
    pub value: T,
    /// The write options of the bytes put, dropped when the sink keeps
    /// none of them.
    pub options: O,
}

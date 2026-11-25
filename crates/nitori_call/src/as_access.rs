//! Values that stand for a source or sink by lending an access to it.
use crate::{ReceiverAccess, ReceiverFamily};

/// A value that lends an access to the source or sink it stands for.
///
/// Every access lends itself. A type of another crate that holds an
/// access, such as a payload that keeps how much of the stream is left
/// for it, implements this trait to be read or written as that access;
/// the accesses themselves remain this crate's own. The access is kept in
/// the value and lent by `&mut`; a value that adapts what it holds keeps
/// the adapted access, such as a [`Composed`](crate::Composed) of its
/// source and its state.
///
/// The access is an associated type of the value, not of the borrow, so a
/// future that holds a lent access of a borrowed source is `Send` whenever
/// the source is: with an access type for each borrow, rustc cannot prove
/// the borrow's bound when it checks the future for `Send`.
///
/// ```
/// use nitori_call::{AsAccess, ReceiverAccess};
///
/// /// Holds the stream it was read from.
/// struct Payload<A> {
///     io: A,
/// }
/// impl<A: ReceiverAccess> AsAccess for Payload<A> {
///     type Family = A::Family;
///     type Access = A;
///     fn access(&mut self) -> &mut A {
///         &mut self.io
///     }
/// }
///
/// fn read_from<S: AsAccess>(mut source: S) {
///     let _access = source.access();
/// }
/// # fn check<A: ReceiverAccess>(io: A, mut slot: A) {
/// read_from(Payload { io });
/// read_from(&mut slot);
/// # }
/// ```
pub trait AsAccess {
    /// The family of the access.
    type Family: ReceiverFamily;
    /// The access lent.
    type Access: ReceiverAccess<Family = Self::Family>;
    /// Lend the access for the borrow's duration.
    fn access(&mut self) -> &mut Self::Access;
}

/// An access lends itself.
impl<A: ReceiverAccess> AsAccess for A {
    type Family = A::Family;
    type Access = A;
    fn access(&mut self) -> &mut A {
        self
    }
}

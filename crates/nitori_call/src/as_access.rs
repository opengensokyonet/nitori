//! Values that stand for a source or sink by lending an access to it.
use crate::{ReceiverAccess, ReceiverFamily};

/// A value that lends an access to the source or sink it stands for.
///
/// Every access lends itself. A type of another crate that holds an
/// access, such as a payload that keeps how much of the stream is left
/// for it, implements this trait to be read or written as that access;
/// the accesses themselves remain this crate's own. The access is
/// returned by value, so a holder can compose it for the borrow from the
/// parts it keeps, as [`Composed::new`](crate::Composed::new) does.
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
///     type Access<'a>
///         = &'a mut A
///     where
///         Self: 'a;
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
    /// The family of the access, the same for every borrow.
    type Family: ReceiverFamily;
    /// The access lent for one borrow.
    type Access<'a>: ReceiverAccess<Family = Self::Family>
    where
        Self: 'a;
    /// Lend the access for the borrow's duration.
    fn access(&mut self) -> Self::Access<'_>;
}

/// An access lends itself.
impl<A: ReceiverAccess + ?Sized> AsAccess for A {
    type Family = A::Family;
    type Access<'a>
        = &'a mut A
    where
        Self: 'a;
    fn access(&mut self) -> &mut A {
        self
    }
}

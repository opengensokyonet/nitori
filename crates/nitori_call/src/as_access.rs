//! Values that stand for a source or sink by lending an access to it.
use crate::{
    LentAccess, ReceiverAccess, ReceiverFamily,
    access::{ReleasePoint, Sealed, ViewVisitor},
};
use core::fmt;

/// An exclusive borrow that exposes the operations of a [`ReceiverAccess`].
///
/// It keeps the underlying access in its owner's storage. For a
/// [`Composed`](crate::Composed) access, the owner retains control of the parent
/// and composition state.
///
/// ```compile_fail,E0599
/// use nitori_call::{AsAccess, Compose, Composed, Lent, ReceiverAccess};
///
/// fn parent<A: ReceiverAccess, S: Compose<Lent<A::Family>> + Unpin>(
///     access: &mut Composed<A, S>,
/// ) {
///     access.access().parent_mut();
/// }
/// ```
///
/// ```compile_fail,E0599
/// use nitori_call::{AsAccess, Compose, Composed, Lent, ReceiverAccess};
///
/// fn state<A: ReceiverAccess, S: Compose<Lent<A::Family>> + Unpin>(
///     access: &mut Composed<A, S>,
/// ) {
///     access.access().state_mut();
/// }
/// ```
pub struct BorrowedAccess<'a, A> {
    inner: &'a mut A,
}

impl<'a, A: ReceiverAccess> BorrowedAccess<'a, A> {
    /// Borrow an access, exposing its receiver operations.
    pub const fn new(inner: &'a mut A) -> Self {
        Self { inner }
    }

    pub(crate) fn into_release_point(self) -> ReleasePoint<'a> {
        self.inner.release_point()
    }
}

impl<A> fmt::Debug for BorrowedAccess<'_, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowedAccess").finish_non_exhaustive()
    }
}

impl<A: ReceiverAccess> Sealed for BorrowedAccess<'_, A> {}

impl<A: ReceiverAccess> ReceiverAccess for BorrowedAccess<'_, A> {
    type Family = A::Family;

    fn visit_view<V: ViewVisitor<A::Family>>(&mut self, visitor: V) -> V::Output {
        self.inner.visit_view(visitor)
    }

    fn release_point(&mut self) -> ReleasePoint<'_> {
        self.inner.release_point()
    }
}

impl<A: LentAccess> LentAccess for BorrowedAccess<'_, A> {}

/// A value that lends an access to the source or sink it stands for.
///
/// Every access lends itself. A type of another crate that holds an
/// access, such as a payload that keeps how much of the stream is left
/// for it, implements this trait to be read or written as that access;
/// the accesses themselves remain this crate's own. The access is kept in
/// the value and lent through a [`BorrowedAccess`]. A value that adapts what
/// it holds keeps the adapted access, such as a [`Composed`](crate::Composed)
/// of its source and its state.
///
/// The access is a fixed associated type of the value. This lets rustc check
/// it for `Send` when a future holds the loan of a borrowed source across an
/// await.
///
/// ```
/// use nitori_call::{AsAccess, BorrowedAccess, ReceiverAccess};
///
/// /// Holds the stream it was read from.
/// struct Payload<A> {
///     io: A,
/// }
/// impl<A: ReceiverAccess> AsAccess for Payload<A> {
///     type Family = A::Family;
///     type Access = A;
///     fn access(&mut self) -> BorrowedAccess<'_, A> {
///         BorrowedAccess::new(&mut self.io)
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
    fn access(&mut self) -> BorrowedAccess<'_, Self::Access>;
}

/// An access lends itself.
impl<A: ReceiverAccess> AsAccess for A {
    type Family = A::Family;
    type Access = A;
    fn access(&mut self) -> BorrowedAccess<'_, A> {
        BorrowedAccess::new(self)
    }
}

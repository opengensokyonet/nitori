//! The receivers of a call: one family, or several, each lent to a slot of
//! its own.
use crate::{
    Lend, ReceiverAccess, ReceiverFamily, ReceiverSlot, ViewLoan,
    access::{ThreadMark, ViewCell},
};
use std::{fmt, pin::Pin, task::Waker};

/// The receivers a call lends views of: one family, or a tuple of them.
///
/// The operation of the call receives a slot for each receiver, and the
/// caller lends each one separately: a view of one receiver can be lent while
/// another is absent, and each slot asks for its own view or offers it back.
pub trait Receivers: cells::Sealed + Sized {
    /// The slots an operation receives: one for each receiver.
    type Slots<'s>: Accesses<Families = Self>;
    /// What the caller lends for one poll: a view for each receiver, if it
    /// has one.
    type Lends<'a>
    where
        Self: 'a;
    /// One flag for each receiver.
    type Flags: Copy + fmt::Debug + PartialEq + Eq;
    #[doc(hidden)]
    type Cells: cells::CellSet<Self>;
}

/// The accesses an operation starts with: one access, or a tuple of them.
pub trait Accesses: accesses::Sealed {
    /// The families of the accesses, in the same shape.
    type Families: Receivers;
}
impl<A: ReceiverAccess> accesses::Sealed for A {}
impl<A: ReceiverAccess> Accesses for A {
    type Families = A::Family;
}
mod accesses {
    pub trait Sealed {}
}

impl<F: ReceiverFamily> cells::Sealed for F {}
impl<F: ReceiverFamily> Receivers for F {
    type Slots<'s> = ReceiverSlot<'s, F>;
    type Lends<'a>
        = Option<Lend<ViewLoan<'a, F>>>
    where
        F: 'a;
    type Flags = bool;
    type Cells = ViewCell<F>;
}

/// A single family's lend, named without a `Receivers` bound in scope, which
/// would keep `Lends` from normalizing.
pub(crate) trait Single: Receivers {
    fn lend<'a, 'view: 'a>(
        view: Option<Lend<Pin<&'a mut <Self as ReceiverFamily>::ReceiverView<'view>>>>,
    ) -> Self::Lends<'a>
    where
        Self: ReceiverFamily + 'view;
}
impl<F: ReceiverFamily> Single for F {
    #[inline]
    fn lend<'a, 'view: 'a>(
        view: Option<Lend<Pin<&'a mut F::ReceiverView<'view>>>>,
    ) -> Option<Lend<ViewLoan<'a, F>>>
    where
        F: 'view,
    {
        view.map(|view| view.map(ViewLoan::new))
    }
}

pub(crate) mod cells {
    use super::*;

    pub trait Sealed {}

    /// The view cells of a call, one for each receiver.
    pub trait CellSet<R: Receivers> {
        fn new() -> Self;
        /// # Safety
        /// As for [`ReceiverSlot::new`], for each cell.
        unsafe fn slots<'s>(&self) -> R::Slots<'s>;
        /// Answer the previous poll's offers and lend this poll's views on
        /// this thread. With `intercepting`, mark the cells that are lent a
        /// view or answered while branches wait for them, and return whether
        /// there are any; without, no branch waits.
        ///
        /// # Safety
        /// The views must outlive the poll, which ends with `clear`.
        unsafe fn lend(&self, lends: R::Lends<'_>, waker: &Waker, intercepting: bool) -> bool;
        /// Move the branches of the cells `lend` marked to `out`.
        fn take_answered(&self, out: &mut Vec<Waker>);
        /// End the poll: no cell lends a view.
        fn clear(&self);
        /// Whether some cell's operation reached a release point with a
        /// releasable view.
        fn offered(&self) -> R::Flags;
        /// Whether some cell's operation asked for a view, or a branch still
        /// waits for one.
        fn needed(&self) -> R::Flags;
        fn any(flags: R::Flags) -> bool;
        /// Whether a branch waits for some cell.
        fn waiting(&self) -> bool;
    }

    impl<F: ReceiverFamily> CellSet<F> for ViewCell<F> {
        #[inline]
        fn new() -> Self {
            ViewCell::new()
        }
        #[inline]
        unsafe fn slots<'s>(&self) -> ReceiverSlot<'s, F> {
            // SAFETY: the caller's contract.
            unsafe { ReceiverSlot::new(self) }
        }
        #[inline]
        unsafe fn lend(
            &self,
            lend: Option<Lend<ViewLoan<'_, F>>>,
            waker: &Waker,
            intercepting: bool,
        ) -> bool {
            // The previous poll ended at a release point; this poll answers
            // by lending the view again or not.
            let offered = self.release.replace(false);
            let lent = lend.is_some();
            self.kept.set(offered && lent);
            // Set only here, and cleared when the branches are taken right
            // after, so it is clear whenever the call does not intercept.
            let answered = intercepting && (offered || lent) && self.waiting();
            if answered {
                self.answered.set(true);
            }
            self.releasable
                .set(lend.as_ref().is_some_and(Lend::is_releasable));
            if let Some(lend) = lend {
                self.view.set(Some(lend.into_inner().into_raw()));
                self.lender.set(ThreadMark::current());
            }
            self.root.set(waker);
            self.requested.set(false);
            answered
        }
        #[cold]
        fn take_answered(&self, out: &mut Vec<Waker>) {
            if self.answered.replace(false) {
                self.take_branches(out);
            }
        }
        #[inline]
        fn clear(&self) {
            self.view.set(None);
            self.lender.set(ThreadMark::NONE);
            self.root.clear();
        }
        #[inline]
        fn offered(&self) -> bool {
            self.release.get()
        }
        #[inline]
        fn needed(&self) -> bool {
            self.requested.get() || self.waiting()
        }
        #[inline]
        fn any(flag: bool) -> bool {
            flag
        }
        #[inline]
        fn waiting(&self) -> bool {
            ViewCell::waiting(self)
        }
    }
}

macro_rules! tuple_receivers {
    ($($name:ident $index:tt),+) => {
        impl<$($name: Receivers),+> cells::Sealed for ($($name,)+) {}
        impl<$($name: Receivers),+> Receivers for ($($name,)+) {
            type Slots<'s> = ($($name::Slots<'s>,)+);
            type Lends<'a>
                = ($($name::Lends<'a>,)+)
            where
                Self: 'a;
            type Flags = ($($name::Flags,)+);
            type Cells = ($($name::Cells,)+);
        }
        impl<$($name: Receivers),+> cells::CellSet<($($name,)+)> for ($($name::Cells,)+) {
            #[inline]
            fn new() -> Self {
                ($(<$name::Cells as cells::CellSet<$name>>::new(),)+)
            }
            #[inline]
            unsafe fn slots<'s>(&self) -> ($($name::Slots<'s>,)+) {
                // SAFETY: the caller's contract, for each cell.
                unsafe { ($(cells::CellSet::<$name>::slots(&self.$index),)+) }
            }
            #[inline]
            unsafe fn lend(
                &self,
                lends: <($($name,)+) as Receivers>::Lends<'_>,
                waker: &Waker,
                intercepting: bool,
            ) -> bool {
                // SAFETY: the caller's contract, for each cell.
                false $(| unsafe { cells::CellSet::<$name>::lend(&self.$index, lends.$index, waker, intercepting) })+
            }
            fn take_answered(&self, out: &mut Vec<Waker>) {
                $(cells::CellSet::<$name>::take_answered(&self.$index, out);)+
            }
            #[inline]
            fn clear(&self) {
                $(cells::CellSet::<$name>::clear(&self.$index);)+
            }
            #[inline]
            fn offered(&self) -> ($($name::Flags,)+) {
                ($(cells::CellSet::<$name>::offered(&self.$index),)+)
            }
            #[inline]
            fn needed(&self) -> ($($name::Flags,)+) {
                ($(cells::CellSet::<$name>::needed(&self.$index),)+)
            }
            #[inline]
            fn any(flags: ($($name::Flags,)+)) -> bool {
                false $(|| <$name::Cells as cells::CellSet<$name>>::any(flags.$index))+
            }
            #[inline]
            fn waiting(&self) -> bool {
                false $(|| cells::CellSet::<$name>::waiting(&self.$index))+
            }
        }
        impl<$($name: Accesses),+> accesses::Sealed for ($($name,)+) {}
        impl<$($name: Accesses),+> Accesses for ($($name,)+) {
            type Families = ($($name::Families,)+);
        }
    };
}
tuple_receivers!(A 0, B 1);
tuple_receivers!(A 0, B 1, C 2);
tuple_receivers!(A 0, B 1, C 2, D 3);

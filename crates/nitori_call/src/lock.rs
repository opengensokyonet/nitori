//! Locks as executors: an operation runs on the value behind a lock, which
//! is locked only on polls whose operation uses it.
use crate::{
    Compose, Drive, Lend, Receiver, ReceiverFamily, RootOperation, ViewAcquisition,
    call::InPlace,
    driver::{CallExecutor, Composing, drive_composed},
};
use std::{
    fmt,
    future::Future,
    marker::PhantomData,
    ops::DerefMut,
    pin::Pin,
    task::{Context, Poll, ready},
};

/// A lock whose guard gives exclusive access to its value, acquired by a
/// future.
///
/// Implemented for `std::sync::Mutex`, for `tokio::sync::Mutex` with the
/// `tokio-sync` feature and for `futures::lock::Mutex` with the
/// `futures-lock` feature. `std::sync::Mutex` blocks the polling thread while
/// another thread holds it, and a poisoned one panics.
pub trait Lock {
    type Value: ?Sized;
    type Guard<'a>: DerefMut<Target = Self::Value> + Unpin
    where
        Self: 'a;
    type Acquire<'a>: Future<Output = Self::Guard<'a>>
    where
        Self: 'a;
    fn acquire(&self) -> Self::Acquire<'_>;
}

/// The family whose view is the guard of lock `L`.
pub struct Locked<L: ?Sized>(PhantomData<fn(&L) -> &L>);
impl<L: Lock + ?Sized> ReceiverFamily for Locked<L> {
    type ReceiverView<'view>
        = L::Guard<'view>
    where
        Self: 'view;
}

/// Locks on demand and delivers releasable guards, so the lock is also
/// released at the release points of operations.
pub struct LockAcquisition<'lock, L: Lock + ?Sized> {
    lock: &'lock L,
    pending: InPlace<Option<L::Acquire<'lock>>>,
}
impl<'lock, L: Lock + ?Sized> LockAcquisition<'lock, L> {
    pub fn new(lock: &'lock L) -> Self {
        Self {
            lock,
            pending: InPlace::new(None),
        }
    }
    fn pending(self: Pin<&mut Self>) -> (&'lock L, Pin<&mut Option<L::Acquire<'lock>>>) {
        // SAFETY: the pending acquisition is structurally pinned and never
        // moved out; the lock reference is not pinned.
        let this = unsafe { self.get_unchecked_mut() };
        (this.lock, unsafe {
            Pin::new_unchecked(this.pending.get_mut())
        })
    }
}
impl<L: Lock + ?Sized> fmt::Debug for LockAcquisition<'_, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LockAcquisition").finish_non_exhaustive()
    }
}
impl<'lock, L: Lock + ?Sized> ViewAcquisition<'lock> for LockAcquisition<'lock, L> {
    type Family = Locked<L>;
    fn poll_acquire(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Lend<L::Guard<'lock>>> {
        let (lock, mut pending) = self.pending();
        if pending.is_none() {
            pending.set(Some(lock.acquire()));
        }
        let guard = ready!(pending.as_mut().as_pin_mut().unwrap().poll(cx));
        pending.set(None);
        Poll::Ready(Lend::Releasable(guard))
    }
    fn advance_unrequested(self: Pin<&mut Self>, cx: &mut Context<'_>) {
        let (_, mut pending) = self.pending();
        if let Some(acquire) = pending.as_mut().as_pin_mut()
            && acquire.poll(cx).is_ready()
        {
            pending.set(None);
        }
    }
}

/// Composes a lock guard into the view of the value behind it, so that
/// operations run on the value's own family with every capability it has.
#[derive(Debug, Default, Clone, Copy)]
pub struct Guarded;
impl<L: Lock + ?Sized> Compose<Locked<L>> for Guarded
where
    L::Value: Receiver + Unpin,
{
    type Family = <L::Value as Receiver>::Family;
    fn compose<'scope, 'parent>(
        self: Pin<&'scope mut Self>,
        guard: Pin<&'scope mut L::Guard<'parent>>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'scope>
    where
        Locked<L>: 'parent,
        Self::Family: 'scope,
        'parent: 'scope,
    {
        Pin::new(&mut **guard.get_mut()).view()
    }
}

/// The execution of an operation on the value behind a lock.
type OnLock<'lock, L, Op> = Drive<'lock, LockAcquisition<'lock, L>, Op, Composing<Guarded>>;

macro_rules! lock_executor {
    ($(#[$attribute:meta])* $mutex:ident :: $($path:ident)::+, $guard:ty, $acquire:ty, |$lock:ident| $body:expr) => {
        $(#[$attribute])*
        impl<T: ?Sized> Lock for $mutex::$($path)::+<T> {
            type Value = T;
            type Guard<'a> = $guard where Self: 'a;
            type Acquire<'a> = $acquire where Self: 'a;
            fn acquire(&self) -> Self::Acquire<'_> {
                let $lock = self;
                $body
            }
        }
        $(#[$attribute])*
        /// Runs operations on the value, locking it only on polls whose
        /// operation uses it and releasing it at the end of each, or at
        /// release points.
        impl<'lock, T: Receiver + Unpin> CallExecutor for &'lock $mutex::$($path)::+<T> {
            type Receivers = T::Family;
            type Execution<'a, Op>
                = OnLock<'lock, $mutex::$($path)::+<T>, Op>
            where
                Self: 'a,
                Op: RootOperation<T::Family> + 'a;
            fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> Self::Execution<'a, Op>
            where
                Op: RootOperation<T::Family> + 'a,
            {
                drive_composed(LockAcquisition::new(*self), Guarded, operation)
            }
        }
    };
}

lock_executor!(
    std::sync::Mutex,
    std::sync::MutexGuard<'a, T>,
    std::future::Ready<std::sync::MutexGuard<'a, T>>,
    |lock| std::future::ready(lock.lock().expect("the lock is poisoned"))
);
lock_executor!(
    #[cfg(feature = "tokio-sync")]
    tokio::sync::Mutex,
    tokio::sync::MutexGuard<'a, T>,
    impl Future<Output = tokio::sync::MutexGuard<'a, T>>,
    |lock| lock.lock()
);
lock_executor!(
    #[cfg(feature = "futures-lock")]
    futures_util::lock::Mutex,
    futures_util::lock::MutexGuard<'a, T>,
    futures_util::lock::MutexLockFuture<'a, T>,
    |lock| lock.lock()
);

//! Receiver identity and optional synchronous view construction.
use std::{marker::PhantomData, pin::Pin};

/// A resource, or a temporary view, with one canonical execution family.
pub trait Receiver {
    type Family: ReceiverFamily;
    /// Construct a bounded view after resource access has been obtained.
    /// View construction may perform work; the resulting value need not be Self.
    fn view<'visit>(
        self: Pin<&'visit mut Self>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'visit>
    where
        Self::Family: 'visit;
}

/// The lifetime-indexed views used by an operation.
pub trait ReceiverFamily: Sized {
    /// A view of the resource for one access lifetime, not necessarily the resource itself.
    type ReceiverView<'visit>: HasReceiverFamily<Family = Self>
    where
        Self: 'visit;
}

/// A family whose views are pinned borrows of one resource type.
pub struct Direct<T: ?Sized>(PhantomData<fn(&T)>);
pub struct DirectView<'visit, T: ?Sized>(pub Pin<&'visit mut T>);
impl<T: ?Sized> ReceiverFamily for Direct<T> {
    type ReceiverView<'visit>
        = DirectView<'visit, T>
    where
        Self: 'visit;
}
impl<T: ?Sized> Receiver for DirectView<'_, T> {
    type Family = Direct<T>;
    fn view<'visit>(self: Pin<&'visit mut Self>) -> DirectView<'visit, T>
    where
        Self::Family: 'visit,
    {
        DirectView(self.get_mut().0.as_mut())
    }
}

/// Associate a local resource with the canonical pinned-borrow family.
#[macro_export]
macro_rules! direct_receiver {
    (impl [$($generics:tt)*] for $ty:ty) => {
        impl<$($generics)*> $crate::Receiver for $ty {
            type Family = $crate::Direct<Self>;
            fn view<'visit>(self: ::core::pin::Pin<&'visit mut Self>) -> $crate::DirectView<'visit, Self>
            where Self::Family: 'visit { $crate::DirectView(self) }
        }
    };
}
direct_receiver!(impl [T] for Vec<T>);
direct_receiver!(impl [T] for std::collections::VecDeque<T>);
direct_receiver!(impl [T] for std::io::Cursor<T>);
direct_receiver!(impl ['data, T] for &'data [T]);
direct_receiver!(impl ['data, T] for &'data mut [T]);
direct_receiver!(impl [] for ());
#[cfg(feature = "bytes")]
direct_receiver!(impl [] for bytes::Bytes);
#[cfg(feature = "bytes")]
direct_receiver!(impl [] for bytes::BytesMut);
impl<T: Receiver + Unpin + ?Sized> Receiver for Box<T> {
    type Family = T::Family;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        Self::Family: 'v,
    {
        Pin::new(&mut **self.get_mut()).view()
    }
}
impl<T: Receiver + Unpin + ?Sized> Receiver for &mut T {
    type Family = T::Family;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        Self::Family: 'v,
    {
        Pin::new(&mut **self.get_mut()).view()
    }
}
impl<P: std::ops::DerefMut + Unpin> Receiver for Pin<P>
where
    P::Target: Receiver,
{
    type Family = <P::Target as Receiver>::Family;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        Self::Family: 'v,
    {
        self.get_mut().as_mut().view()
    }
}
direct_receiver!(impl [] for usize);
direct_receiver!(impl [] for u8);
direct_receiver!(impl [] for u32);
direct_receiver!(impl [] for u64);
direct_receiver!(impl [] for i32);
direct_receiver!(impl [T] for std::cell::Cell<T>);

/// A pinned resource borrow that keeps the resource's canonical family.
pub struct BorrowedReceiver<'visit, H: Receiver + ?Sized>(pub Pin<&'visit mut H>);
impl<H: Receiver + ?Sized> Receiver for BorrowedReceiver<'_, H> {
    type Family = H::Family;
    fn view<'visit>(
        self: Pin<&'visit mut Self>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'visit>
    where
        Self::Family: 'visit,
    {
        self.get_mut().0.as_mut().view()
    }
}
/// Use a local resource type itself as its family marker.
/// Capability implementations can then live beside that resource without
/// running into the orphan rules for external generic family wrappers.
#[macro_export]
macro_rules! family_receiver {
    (impl [$($generics:tt)*] for $ty:ty) => {
        impl<$($generics)*> $crate::ReceiverFamily for $ty {
            type ReceiverView<'visit> = $crate::BorrowedReceiver<'visit,Self> where Self: 'visit;
        }
        impl<$($generics)*> $crate::Receiver for $ty {
            type Family = Self;
            fn view<'visit>(self: ::core::pin::Pin<&'visit mut Self>) -> $crate::BorrowedReceiver<'visit,Self>
            where Self: 'visit { $crate::BorrowedReceiver(self) }
        }
    };
}
direct_receiver!(impl [T: ?Sized] for std::rc::Rc<T>);
direct_receiver!(impl [] for bool);

/// Capability identity independent of view construction.
pub trait HasReceiverFamily {
    type Family: ReceiverFamily;
}
impl<T: Receiver + ?Sized> HasReceiverFamily for T {
    type Family = T::Family;
}

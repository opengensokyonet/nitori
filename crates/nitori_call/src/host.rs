//! Stable families and freshly reconstructed host views.
use std::{marker::PhantomData, pin::Pin};

/// A resource, or a temporary view, with one canonical execution family.
pub trait Host {
    type Family: HostFamily;
    fn view<'visit>(self: Pin<&'visit mut Self>) -> <Self::Family as HostFamily>::Host<'visit>
    where
        Self::Family: 'visit;
}

/// The lifetime-indexed hosts used by an operation.
pub trait HostFamily: Sized {
    type Host<'visit>: Host<Family = Self>
    where
        Self: 'visit;
}

/// A family whose views are pinned borrows of one resource type.
pub struct Direct<T: ?Sized>(PhantomData<fn(&T)>);
pub struct DirectView<'visit, T: ?Sized>(pub Pin<&'visit mut T>);
impl<T: ?Sized> HostFamily for Direct<T> {
    type Host<'visit>
        = DirectView<'visit, T>
    where
        Self: 'visit;
}
impl<T: ?Sized> Host for DirectView<'_, T> {
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
macro_rules! direct_host {
    (impl [$($generics:tt)*] for $ty:ty) => {
        impl<$($generics)*> $crate::Host for $ty {
            type Family = $crate::Direct<Self>;
            fn view<'visit>(self: ::core::pin::Pin<&'visit mut Self>) -> $crate::DirectView<'visit, Self>
            where Self::Family: 'visit { $crate::DirectView(self) }
        }
    };
}
direct_host!(impl [T] for Vec<T>);
direct_host!(impl [T] for std::collections::VecDeque<T>);
direct_host!(impl [T] for std::io::Cursor<T>);
direct_host!(impl ['data, T] for &'data [T]);
direct_host!(impl ['data, T] for &'data mut [T]);
direct_host!(impl [] for ());
#[cfg(feature = "bytes")]
direct_host!(impl [] for bytes::Bytes);
#[cfg(feature = "bytes")]
direct_host!(impl [] for bytes::BytesMut);
impl<T: Host + Unpin + ?Sized> Host for Box<T> {
    type Family = T::Family;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as HostFamily>::Host<'v>
    where
        Self::Family: 'v,
    {
        Pin::new(&mut **self.get_mut()).view()
    }
}
impl<T: Host + Unpin + ?Sized> Host for &mut T {
    type Family = T::Family;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as HostFamily>::Host<'v>
    where
        Self::Family: 'v,
    {
        Pin::new(&mut **self.get_mut()).view()
    }
}
impl<P: std::ops::DerefMut + Unpin> Host for Pin<P>
where
    P::Target: Host,
{
    type Family = <P::Target as Host>::Family;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as HostFamily>::Host<'v>
    where
        Self::Family: 'v,
    {
        self.get_mut().as_mut().view()
    }
}
direct_host!(impl [] for usize);
direct_host!(impl [] for u8);
direct_host!(impl [] for u32);
direct_host!(impl [] for u64);
direct_host!(impl [] for i32);
direct_host!(impl [T] for std::cell::Cell<T>);

/// A pinned resource borrow that keeps the resource's canonical family.
pub struct BorrowedHost<'visit, H: Host + ?Sized>(pub Pin<&'visit mut H>);
impl<H: Host + ?Sized> Host for BorrowedHost<'_, H> {
    type Family = H::Family;
    fn view<'visit>(self: Pin<&'visit mut Self>) -> <Self::Family as HostFamily>::Host<'visit>
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
macro_rules! family_host {
    (impl [$($generics:tt)*] for $ty:ty) => {
        impl<$($generics)*> $crate::HostFamily for $ty {
            type Host<'visit> = $crate::BorrowedHost<'visit,Self> where Self: 'visit;
        }
        impl<$($generics)*> $crate::Host for $ty {
            type Family = Self;
            fn view<'visit>(self: ::core::pin::Pin<&'visit mut Self>) -> $crate::BorrowedHost<'visit,Self>
            where Self: 'visit { $crate::BorrowedHost(self) }
        }
    };
}
direct_host!(impl [T: ?Sized] for std::rc::Rc<T>);
direct_host!(impl [] for bool);

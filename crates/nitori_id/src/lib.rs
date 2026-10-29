#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use core::{
    any::TypeId,
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

/// A value that no other allocation in this process returns.
///
/// Each [`UniqueId::allocate`] call returns a value different from every
/// other `UniqueId` in the process. Equality, hashing and ordering compare
/// that value; the order is total but its meaning is unspecified. The value
/// itself is not exposed and cannot be rebuilt from a number, so a `UniqueId`
/// is meaningful only within the process that allocated it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UniqueId(u64);

impl UniqueId {
    /// Allocate a value distinct from every earlier allocation.
    ///
    /// Values are never reused.
    ///
    /// # Panics
    ///
    /// Panics rather than repeat a value once the process has made 2^64
    /// allocations, which at one per nanosecond would take more than 580
    /// years.
    #[must_use]
    pub fn allocate() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // Uniqueness is the only requirement: no other memory is published
        // through the counter, so relaxed ordering suffices. Unlike
        // `fetch_add`, the exchange leaves the counter at its last value once
        // it is exhausted instead of wrapping it to an earlier one.
        let mut current = NEXT.load(Ordering::Relaxed);
        loop {
            let next = current
                .checked_add(1)
                .expect("process-unique values are exhausted");
            match NEXT.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return Self(current),
                Err(actual) => current = actual,
            }
        }
    }
}

impl fmt::Debug for UniqueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("UniqueId").field(&self.0).finish()
    }
}

/// An identity unique within the process, labelled with the type that owns
/// it.
///
/// An `Identity` pairs the [`TypeId`] of a type parameter with a newly
/// allocated [`UniqueId`]. Equality, hashing and ordering compare that pair
/// and nothing else, so they agree with one another; the order is total but
/// its meaning is unspecified.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Identity {
    type_id: TypeId,
    unique: UniqueId,
}

impl Identity {
    /// Allocate a new identity owned by type `T`.
    ///
    /// `T` labels the identity and is otherwise unused; no value of it is
    /// needed or kept.
    ///
    /// # Panics
    ///
    /// Panics if [`UniqueId::allocate`] does.
    #[must_use]
    pub fn new<T: ?Sized + 'static>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            unique: UniqueId::allocate(),
        }
    }

    /// The [`TypeId`] of the type that owns this identity.
    ///
    /// This is the `TypeId` of the type parameter given to [`Identity::new`],
    /// not that of `Identity` itself.
    #[must_use]
    pub fn owner_type_id(&self) -> TypeId {
        self.type_id
    }
}

/// Programs that must not compile.
///
/// Neither type is `Copy`, so that their representation can grow:
///
/// ```compile_fail
/// fn copy<T: Copy>() {}
/// copy::<nitori_id::UniqueId>();
/// ```
///
/// ```compile_fail
/// fn copy<T: Copy>() {}
/// copy::<nitori_id::Identity>();
/// ```
///
/// Neither can be built from a number or by default:
///
/// ```compile_fail
/// let _: nitori_id::UniqueId = 0u64.into();
/// ```
///
/// ```compile_fail
/// let _: nitori_id::UniqueId = Default::default();
/// ```
///
/// ```compile_fail
/// let _: nitori_id::Identity = Default::default();
/// ```
#[cfg(doctest)]
pub struct CompileFail;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_separates_equal_unique_values() {
        let identity = Identity::new::<u8>();
        let other_type = Identity {
            type_id: TypeId::of::<u16>(),
            unique: identity.unique.clone(),
        };
        assert_ne!(identity, other_type);
        assert_ne!(identity.cmp(&other_type), core::cmp::Ordering::Equal);
    }
}

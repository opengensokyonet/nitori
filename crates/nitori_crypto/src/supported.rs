use core::{fmt, hash, marker::PhantomData};

/// A type that answers which algorithms it supports by issuing [`Supported`]
/// values.
///
/// `Token` is the proof that a [`Supported`] value comes from the
/// implementation: [`Supported::new`] requires one. An implementation keeps
/// its token type out of reach of other crates, so that it alone issues
/// values for its type. Support is a property of the type: when one value of
/// the type issues a `Supported` for an algorithm, every value of the type
/// accepts it.
pub trait Issuer {
    /// A value only the implementing crate can create.
    type Token;
}

/// An implementation's answer that it supports `algorithm`.
///
/// Only the implementation `C` issues these values, when asked whether it
/// supports an algorithm; operations that take one do not fail for lack of
/// support. The value holds the algorithm and marks `C` by type alone: it
/// holds no value of `C`, and a value issued by one type is not accepted by
/// another.
pub struct Supported<C: ?Sized, A> {
    algorithm: A,
    backend: PhantomData<fn() -> C>,
}

impl<C: Issuer + ?Sized, A> Supported<C, A> {
    /// Record that `C` supports `algorithm`.
    ///
    /// Requires the token of `C`, so only the implementation of `C` calls
    /// this, and only for an algorithm it supports.
    #[must_use]
    pub fn new(token: C::Token, algorithm: A) -> Self {
        // The token only proves who the caller is.
        drop(token);
        Self {
            algorithm,
            backend: PhantomData,
        }
    }
}

impl<C: ?Sized, A: Copy> Supported<C, A> {
    /// The supported algorithm.
    #[must_use]
    pub const fn algorithm(&self) -> A {
        self.algorithm
    }
}

impl<C: ?Sized, A: Clone> Clone for Supported<C, A> {
    fn clone(&self) -> Self {
        Self {
            algorithm: self.algorithm.clone(),
            backend: PhantomData,
        }
    }
}

impl<C: ?Sized, A: Copy> Copy for Supported<C, A> {}

impl<C: ?Sized, A: PartialEq> PartialEq for Supported<C, A> {
    fn eq(&self, other: &Self) -> bool {
        self.algorithm == other.algorithm
    }
}

impl<C: ?Sized, A: Eq> Eq for Supported<C, A> {}

impl<C: ?Sized, A: hash::Hash> hash::Hash for Supported<C, A> {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.algorithm.hash(state);
    }
}

impl<C: ?Sized, A: fmt::Debug> fmt::Debug for Supported<C, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Supported").field(&self.algorithm).finish()
    }
}

//! The cryptographic capabilities the TLS layer needs from a backend.
//!
//! [`TlsCrypto`] names the set: hashing, HMAC, HKDF, key exchange,
//! signature verification and random bytes, each a trait of
//! `nitori_crypto`. Every type that implements all of them satisfies it, so
//! a backend that combines several libraries, delegating each capability,
//! satisfies it as well. Signing is not part of the set; it belongs to the
//! credential that signs.
//!
//! The capabilities share method names, such as `start` of `Hash` and of
//! `KeyExchange`; where a call is ambiguous on a `TlsCrypto` type, it names
//! the trait, as in `Hash::start(&backend, hash)`.

use nitori_crypto::{Hash, Hkdf, Hmac, KeyExchange, Random, SignatureVerify};

/// The capabilities a backend provides to the TLS layer.
pub trait TlsCrypto: Hash + Hmac + Hkdf + KeyExchange + SignatureVerify + Random {}

impl<T: Hash + Hmac + Hkdf + KeyExchange + SignatureVerify + Random> TlsCrypto for T {}

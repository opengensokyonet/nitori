#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod hash;
mod hkdf;
mod hmac;
mod key_exchange;
mod random;
mod secret;
mod signature;
mod supported;

pub use hash::{Digest, DigestLengthMismatch, Hash, HashAlgorithm, RunningHash};
pub use hkdf::{ExpandLength, ExpandLengthOutOfRange, Hkdf};
pub use hmac::{Hmac, TagMismatch};
pub use key_exchange::{
    CompleteError, DecapsulationFailed, DecapsulationFailedSnafu, InvalidPeerShare,
    InvalidPeerShareSnafu, KeyExchange, KeyGenerationFailed, KeyGenerationFailedSnafu, KeyShare,
    NamedGroup, ServerShareError, StartedExchange,
};
pub use random::{Random, RandomError};
pub use secret::{Secret, SecretTooLong};
pub use signature::{
    BadSignature, SchemeNotForKey, SchemeNotForKeySnafu, Signature, SignatureScheme,
    SignatureVerify, SigningKey,
};
pub use supported::{Issuer, Supported};

/// Programs that must not compile.
///
/// A [`Supported`] issued for one type is not accepted for another:
///
/// ```compile_fail
/// use nitori_crypto::{Issuer, Supported};
///
/// struct First;
/// struct Second;
/// impl Issuer for First {
///     type Token = ();
/// }
/// impl Issuer for Second {
///     type Token = ();
/// }
///
/// let first: Supported<First, u8> = Supported::new((), 0);
/// let _: Supported<Second, u8> = first;
/// ```
///
/// A [`Secret`] cannot be cloned:
///
/// ```compile_fail
/// let secret = nitori_crypto::Secret::new(&[1]).unwrap();
/// let _ = secret.clone();
/// ```
#[cfg(doctest)]
pub struct CompileFail;

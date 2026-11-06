#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod hash;
mod key_exchange;
mod signing;
mod verify;

use nitori_crypto::{Issuer, Random, RandomError};
use ring::rand::{SecureRandom, SystemRandom};

pub use hash::RingRunningHash;
pub use key_exchange::RingStartedExchange;
pub use signing::{
    EcdsaKeyRejectedError, EcdsaSigningError, EcdsaSigningKey, Ed25519KeyRejectedError,
    Ed25519SigningKey, RsaKeyRejectedError, RsaSigningError, RsaSigningKey,
};

/// The capabilities of `nitori_crypto` that ring provides.
///
/// - `Hash`, `Hmac` and `Hkdf` with SHA-256 and SHA-384.
/// - `KeyExchange` with X25519, P-256 and P-384.
/// - `SignatureVerify` with ECDSA over P-256 and P-384, Ed25519, RSA-PSS
///   and RSASSA-PKCS1-v1_5, for RSA moduli of 2048 to 8192 bits.
/// - `Random` from the operating system's generator.
///
/// The type holds no state. Local signing keys are separate types:
/// [`EcdsaSigningKey`], [`Ed25519SigningKey`] and [`RsaSigningKey`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RingBackend;

mod token {
    /// The proof that a `Supported` value comes from this crate.
    pub struct Token(());

    impl Token {
        pub(crate) fn new() -> Self {
            Self(())
        }
    }
}

impl Issuer for RingBackend {
    type Token = token::Token;
}

impl Random for RingBackend {
    fn random<const N: usize>(&self) -> Result<[u8; N], RandomError> {
        let mut out = [0; N];
        // ring reports no detail beyond the failure itself.
        SystemRandom::new()
            .fill(&mut out)
            .map_err(|_| RandomError)?;
        Ok(out)
    }
}

/// Programs that must not compile.
///
/// Other crates cannot issue a `Supported` for [`RingBackend`]:
///
/// ```compile_fail
/// use nitori_crypto::{NamedGroup, Supported};
/// use nitori_crypto_ring::RingBackend;
///
/// let _ = Supported::<RingBackend, _>::new(nitori_crypto_ring::token::Token::new(), NamedGroup::new(0x9999));
/// ```
///
/// Completing a started exchange consumes it:
///
/// ```compile_fail
/// use nitori_crypto::{KeyExchange, NamedGroup, StartedExchange};
/// use nitori_crypto_ring::RingBackend;
///
/// let backend = RingBackend;
/// let group = backend.named_group(NamedGroup::X25519).unwrap();
/// let client = KeyExchange::start(&backend, group).unwrap();
/// let _ = client.complete(&[9; 32]);
/// let _ = client.share();
/// ```
#[cfg(doctest)]
pub struct CompileFail;

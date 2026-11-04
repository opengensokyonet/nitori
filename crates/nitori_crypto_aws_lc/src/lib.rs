#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

#[cfg(feature = "fixed-x25519")]
mod fixed;
mod hash;
mod key_exchange;
mod signing;
mod verify;

use aws_lc_rs::rand;
use nitori_crypto::{Issuer, Random, RandomError};

#[cfg(feature = "fixed-x25519")]
pub use fixed::FixedX25519;
pub use hash::AwsLcRunningHash;
pub use key_exchange::AwsLcStartedExchange;
pub use signing::{
    EcdsaKeyRejected, EcdsaSigningError, EcdsaSigningKey, Ed25519KeyRejected, Ed25519SigningKey,
    RsaKeyRejected, RsaSigningError, RsaSigningKey,
};

/// The capabilities of `nitori_crypto` that aws-lc-rs provides.
///
/// - `Hash`, `Hmac` and `Hkdf` with SHA-256 and SHA-384.
/// - `KeyExchange` with X25519MLKEM768, X25519, P-256, P-384, P-521 and
///   ML-KEM-768.
/// - `SignatureVerify` with ECDSA over P-256, P-384 and P-521, Ed25519,
///   RSA-PSS and RSASSA-PKCS1-v1_5, for RSA moduli of 2048 to 8192 bits.
/// - `Random` from AWS-LC's generator.
///
/// The type holds no state. Local signing keys are separate types:
/// [`EcdsaSigningKey`], [`Ed25519SigningKey`] and [`RsaSigningKey`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AwsLcBackend;

mod token {
    /// The proof that a `Supported` value comes from this crate.
    pub struct Token(());

    impl Token {
        pub(crate) fn new() -> Self {
            Self(())
        }
    }
}

impl Issuer for AwsLcBackend {
    type Token = token::Token;
}

impl Random for AwsLcBackend {
    fn random<const N: usize>(&self) -> Result<[u8; N], RandomError> {
        let mut out = [0; N];
        // aws-lc-rs reports no detail beyond the failure itself.
        rand::fill(&mut out).map_err(|_| RandomError)?;
        Ok(out)
    }
}

/// Programs that must not compile.
///
/// Other crates cannot issue a `Supported` for [`AwsLcBackend`]:
///
/// ```compile_fail
/// use nitori_crypto::{NamedGroup, Supported};
/// use nitori_crypto_aws_lc::AwsLcBackend;
///
/// let _ = Supported::<AwsLcBackend, _>::new(nitori_crypto_aws_lc::token::Token::new(), NamedGroup::new(0x9999));
/// ```
///
/// Completing a started exchange consumes it:
///
/// ```compile_fail
/// use nitori_crypto::{KeyExchange, NamedGroup, StartedExchange};
/// use nitori_crypto_aws_lc::AwsLcBackend;
///
/// let backend = AwsLcBackend;
/// let group = backend.named_group(NamedGroup::X25519).unwrap();
/// let client = KeyExchange::start(&backend, group).unwrap();
/// let _ = client.complete(&[9; 32]);
/// let _ = client.share();
/// ```
#[cfg(doctest)]
pub struct CompileFail;

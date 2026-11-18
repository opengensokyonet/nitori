//! TLS 1.3 cipher suites (RFC 9846 §B.4).
//!
//! A cipher suite names the AEAD algorithm that protects records, in QUIC
//! packets, and the hash used for the transcript and HKDF.

use crate::code_point::code_point;
use core::fmt;
use nitori_crypto::{AeadAlgorithm, Hash, HashAlgorithm, Supported};

code_point! {
    /// A CipherSuite code point.
    ///
    /// Any two-byte value is a code point; the readers keep suites they do
    /// not know, which a server ignores (RFC 9846 §9.3).
    CipherSuite(u16);
    /// TLS_AES_128_GCM_SHA256.
    TLS_AES_128_GCM_SHA256 = 0x1301 => "TLS_AES_128_GCM_SHA256",
    /// TLS_AES_256_GCM_SHA384.
    TLS_AES_256_GCM_SHA384 = 0x1302 => "TLS_AES_256_GCM_SHA384",
    /// TLS_CHACHA20_POLY1305_SHA256.
    TLS_CHACHA20_POLY1305_SHA256 = 0x1303 => "TLS_CHACHA20_POLY1305_SHA256",
    /// TLS_AES_128_CCM_SHA256.
    TLS_AES_128_CCM_SHA256 = 0x1304 => "TLS_AES_128_CCM_SHA256",
    /// TLS_AES_128_CCM_8_SHA256, which QUIC does not use (RFC 9001 §5.3).
    TLS_AES_128_CCM_8_SHA256 = 0x1305 => "TLS_AES_128_CCM_8_SHA256",
}

impl CipherSuite {
    /// The hash of a TLS 1.3 suite, or `None` for a suite this crate does
    /// not name.
    pub const fn hash(self) -> Option<HashAlgorithm> {
        match self {
            Self::TLS_AES_128_GCM_SHA256
            | Self::TLS_CHACHA20_POLY1305_SHA256
            | Self::TLS_AES_128_CCM_SHA256
            | Self::TLS_AES_128_CCM_8_SHA256 => Some(HashAlgorithm::Sha256),
            Self::TLS_AES_256_GCM_SHA384 => Some(HashAlgorithm::Sha384),
            _ => None,
        }
    }

    /// The AEAD algorithm of the suite, or `None` for a suite whose AEAD
    /// [`AeadAlgorithm`] does not name, the CCM suites among them.
    pub const fn aead(self) -> Option<AeadAlgorithm> {
        match self {
            Self::TLS_AES_128_GCM_SHA256 => Some(AeadAlgorithm::Aes128Gcm),
            Self::TLS_AES_256_GCM_SHA384 => Some(AeadAlgorithm::Aes256Gcm),
            Self::TLS_CHACHA20_POLY1305_SHA256 => Some(AeadAlgorithm::ChaCha20Poly1305),
            _ => None,
        }
    }
}

/// A cipher suite a backend can run: the backend's answer that it supports
/// the suite's hash, with the suite's AEAD algorithm.
///
/// The transcript, HKDF and HMAC take the hash's [`Supported`], so nothing
/// after the choice of a suite fails for lack of support.
pub struct SupportedSuite<C> {
    suite: CipherSuite,
    hash: Supported<C, HashAlgorithm>,
    aead: AeadAlgorithm,
}

impl<C: Hash> SupportedSuite<C> {
    /// `suite` as `backend` runs it, or `None` when the suite is not a
    /// TLS 1.3 suite with an AEAD that [`AeadAlgorithm`] names or the
    /// backend does not support its hash.
    pub fn new(backend: &C, suite: CipherSuite) -> Option<Self> {
        let hash = backend.hash_algorithm(suite.hash()?)?;
        let aead = suite.aead()?;
        Some(Self { suite, hash, aead })
    }
}

impl<C> SupportedSuite<C> {
    /// The suite.
    pub fn suite(&self) -> CipherSuite {
        self.suite
    }

    /// The backend's support for the suite's hash.
    pub fn hash(&self) -> Supported<C, HashAlgorithm> {
        self.hash
    }

    /// The AEAD algorithm of the suite.
    pub fn aead(&self) -> AeadAlgorithm {
        self.aead
    }
}

impl<C> Clone for SupportedSuite<C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C> Copy for SupportedSuite<C> {}

impl<C> PartialEq for SupportedSuite<C> {
    fn eq(&self, other: &Self) -> bool {
        self.suite == other.suite
    }
}

impl<C> Eq for SupportedSuite<C> {}

impl<C> fmt::Debug for SupportedSuite<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SupportedSuite").field(&self.suite).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{CipherSuite, SupportedSuite};
    use nitori_crypto::{AeadAlgorithm, HashAlgorithm};
    use nitori_crypto_aws_lc::AwsLcBackend;

    #[test]
    fn suites_map_to_their_hash_and_aead() {
        let suite = CipherSuite::TLS_AES_256_GCM_SHA384;
        assert_eq!(suite.hash(), Some(HashAlgorithm::Sha384));
        assert_eq!(suite.aead(), Some(AeadAlgorithm::Aes256Gcm));
        let ccm = CipherSuite::TLS_AES_128_CCM_SHA256;
        assert_eq!(
            (ccm.hash(), ccm.aead()),
            (Some(HashAlgorithm::Sha256), None)
        );
        assert_eq!(CipherSuite::new(0x1306).hash(), None);
        assert_eq!(
            format!("{:?}", CipherSuite::TLS_AES_128_GCM_SHA256),
            "CipherSuite(TLS_AES_128_GCM_SHA256, 0x1301)"
        );
        assert_eq!(
            format!("{:?}", CipherSuite::new(0x0a0a)),
            "CipherSuite(0xa0a)"
        );
    }

    #[test]
    fn a_suite_is_supported_with_its_hash_and_a_named_aead() {
        let backend = AwsLcBackend;
        let suite = SupportedSuite::new(&backend, CipherSuite::TLS_AES_256_GCM_SHA384).unwrap();
        assert_eq!(suite.hash().algorithm(), HashAlgorithm::Sha384);
        assert_eq!(suite.aead(), AeadAlgorithm::Aes256Gcm);
        assert!(SupportedSuite::new(&backend, CipherSuite::TLS_AES_128_CCM_SHA256).is_none());
        assert!(SupportedSuite::new(&backend, CipherSuite::new(0x1306)).is_none());
    }
}

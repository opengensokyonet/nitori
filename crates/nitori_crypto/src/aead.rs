use snafu::Snafu;

use crate::{Issuer, Supported};

/// An AEAD algorithm of a TLS 1.3 cipher suite (RFC 9846 §B.4).
///
/// Every algorithm here takes a nonce of [`AeadAlgorithm::NONCE_LEN`] bytes
/// and produces a tag of [`AeadAlgorithm::TAG_LEN`] bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AeadAlgorithm {
    /// `AEAD_AES_128_GCM` (RFC 5116 §5.1).
    Aes128Gcm,
    /// `AEAD_AES_256_GCM` (RFC 5116 §5.2).
    Aes256Gcm,
    /// `AEAD_CHACHA20_POLY1305` (RFC 8439 §2.8).
    ChaCha20Poly1305,
}

impl AeadAlgorithm {
    /// The length of the nonce in bytes.
    pub const NONCE_LEN: usize = 12;

    /// The length of the authentication tag in bytes.
    pub const TAG_LEN: usize = 16;

    /// The length of the key in bytes.
    #[must_use]
    pub const fn key_len(self) -> usize {
        match self {
            Self::Aes128Gcm => 16,
            Self::Aes256Gcm | Self::ChaCha20Poly1305 => 32,
        }
    }
}

/// Authenticated encryption with associated data.
///
/// [`Aead::aead_algorithm`] answers which algorithms the implementation
/// supports; [`Aead::aead_key`] makes a key for a supported one.
pub trait Aead: Issuer {
    /// A key ready for sealing and opening.
    type Key: AeadKey;

    /// Whether this implementation supports `algorithm`.
    fn aead_algorithm(&self, algorithm: AeadAlgorithm) -> Option<Supported<Self, AeadAlgorithm>>;

    /// The key of `algorithm` with the bytes `key`.
    ///
    /// # Errors
    ///
    /// Fails when `key` is not [`AeadAlgorithm::key_len`] bytes long.
    fn aead_key(
        &self,
        algorithm: Supported<Self, AeadAlgorithm>,
        key: &[u8],
    ) -> Result<Self::Key, AeadKeyLengthMismatchError>;
}

/// An AEAD key that seals and opens in place, with the tag kept apart from
/// the text.
///
/// The caller chooses the nonce. A nonce must not be used twice with one
/// key: doing so breaks the confidentiality and, for the algorithms here,
/// the integrity of every message sealed with that key.
pub trait AeadKey {
    /// Encrypt `in_out` in place, authenticating it together with `aad`,
    /// and return the tag.
    ///
    /// # Errors
    ///
    /// Fails without writing `in_out` when it is longer than the algorithm
    /// protects under one nonce, more than 64 GiB for every algorithm here.
    fn seal_in_place(
        &self,
        nonce: &[u8; AeadAlgorithm::NONCE_LEN],
        aad: &[u8],
        in_out: &mut [u8],
    ) -> Result<[u8; AeadAlgorithm::TAG_LEN], AeadInputTooLongError>;

    /// Check `tag` over `aad` and the ciphertext `in_out`, and decrypt
    /// `in_out` in place.
    ///
    /// # Errors
    ///
    /// - [`AeadOpenError::InputTooLong`] without writing `in_out` when it is
    ///   longer than the algorithm protects under one nonce.
    /// - [`AeadOpenError::Authentication`] when the tag does not match. The
    ///   contents of `in_out` are then unspecified: an implementation may
    ///   leave the ciphertext, partly decrypted text or zeros there.
    fn open_in_place(
        &self,
        nonce: &[u8; AeadAlgorithm::NONCE_LEN],
        aad: &[u8],
        in_out: &mut [u8],
        tag: &[u8; AeadAlgorithm::TAG_LEN],
    ) -> Result<(), AeadOpenError>;
}

/// The bytes given for an AEAD key are not as long as the algorithm's key.
#[derive(Debug, Snafu)]
#[snafu(display(
    "key of {len} bytes does not match the {} byte key of {algorithm:?}",
    algorithm.key_len()
))]
pub struct AeadKeyLengthMismatchError {
    algorithm: AeadAlgorithm,
    len: usize,
}

impl AeadKeyLengthMismatchError {
    /// Check that `key` is as long as the key of `algorithm`.
    ///
    /// # Errors
    ///
    /// Fails when the lengths differ.
    pub fn check(algorithm: AeadAlgorithm, key: &[u8]) -> Result<(), Self> {
        if key.len() == algorithm.key_len() {
            Ok(())
        } else {
            AeadKeyLengthMismatchSnafu {
                algorithm,
                len: key.len(),
            }
            .fail()
        }
    }

    /// The algorithm whose key length was expected.
    #[must_use]
    pub fn algorithm(&self) -> AeadAlgorithm {
        self.algorithm
    }

    /// The length that was given, never the key length of
    /// [`AeadKeyLengthMismatchError::algorithm`].
    #[must_use]
    pub fn requested_len(&self) -> usize {
        self.len
    }
}

/// The text given to an AEAD key is longer than its algorithm protects under
/// one nonce.
#[derive(Debug, Snafu)]
#[snafu(display("input of {len} bytes exceeds the {max_len} byte limit of {algorithm:?}"))]
pub struct AeadInputTooLongError {
    algorithm: AeadAlgorithm,
    len: usize,
    max_len: u64,
}

impl AeadInputTooLongError {
    /// The longest text `algorithm` protects under one nonce: 2<sup>32</sup>
    /// − 2 blocks of 16 bytes for AES-GCM (NIST SP 800-38D §5.2.1.1),
    /// 2<sup>32</sup> − 1 blocks of 64 bytes for ChaCha20-Poly1305
    /// (RFC 8439 §2.8).
    #[must_use]
    pub const fn max_len(algorithm: AeadAlgorithm) -> u64 {
        match algorithm {
            AeadAlgorithm::Aes128Gcm | AeadAlgorithm::Aes256Gcm => ((1 << 32) - 2) * 16,
            AeadAlgorithm::ChaCha20Poly1305 => ((1 << 32) - 1) * 64,
        }
    }

    /// Check that a text of `len` bytes is within the limit of `algorithm`.
    ///
    /// # Errors
    ///
    /// Fails when it is longer than [`AeadInputTooLongError::max_len`].
    pub fn check(algorithm: AeadAlgorithm, len: usize) -> Result<(), Self> {
        let max_len = Self::max_len(algorithm);
        // A `usize` that does not fit a `u64` exceeds every limit.
        if u64::try_from(len).is_ok_and(|len| len <= max_len) {
            Ok(())
        } else {
            AeadInputTooLongSnafu {
                algorithm,
                len,
                max_len,
            }
            .fail()
        }
    }

    /// The algorithm whose limit was crossed.
    #[must_use]
    pub fn algorithm(&self) -> AeadAlgorithm {
        self.algorithm
    }

    /// The length that was given, always above the limit.
    #[must_use]
    pub fn requested_len(&self) -> usize {
        self.len
    }
}

/// A tag does not authenticate the ciphertext and the associated data.
#[derive(Debug, Snafu)]
#[snafu(display("aead tag does not match"))]
pub struct AeadAuthenticationError;

/// The error of [`AeadKey::open_in_place`].
#[derive(Debug, Snafu)]
pub enum AeadOpenError {
    /// The ciphertext is longer than the algorithm protects under one
    /// nonce.
    #[snafu(transparent)]
    InputTooLong {
        /// The length check that failed.
        source: AeadInputTooLongError,
    },
    /// The tag does not match.
    #[snafu(transparent)]
    Authentication {
        /// The failed authentication.
        source: AeadAuthenticationError,
    },
}

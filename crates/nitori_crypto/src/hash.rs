use core::fmt;

use snafu::Snafu;

use crate::{Issuer, Supported};

/// A hash function used by a TLS 1.3 cipher suite, for the transcript, HMAC
/// and HKDF.
///
/// TLS 1.3 names no hash on the wire; the cipher suite determines it, so the
/// set is closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HashAlgorithm {
    /// SHA-256, with a 32-byte output.
    Sha256,
    /// SHA-384, with a 48-byte output.
    Sha384,
}

impl HashAlgorithm {
    /// The length of the hash output in bytes.
    #[must_use]
    pub const fn output_len(self) -> usize {
        match self {
            Self::Sha256 => 32,
            Self::Sha384 => 48,
        }
    }
}

/// The output of a hash function or of HMAC.
///
/// Its length is the output length of the [`HashAlgorithm`] that produced
/// it. A digest is not secret: it can be copied, and `Debug` prints it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest {
    bytes: [u8; Digest::CAPACITY],
    len: u8,
}

impl Digest {
    /// The longest output of any [`HashAlgorithm`].
    const CAPACITY: usize = 48;

    /// Copy the output of `algorithm` into a digest.
    ///
    /// # Errors
    ///
    /// Fails when the length of `bytes` is not the output length of
    /// `algorithm`.
    pub fn new(algorithm: HashAlgorithm, bytes: &[u8]) -> Result<Self, DigestLengthMismatchError> {
        let expected = algorithm.output_len();
        if bytes.len() != expected {
            return DigestLengthMismatchSnafu {
                algorithm,
                len: bytes.len(),
            }
            .fail();
        }
        let mut digest = Self {
            bytes: [0; Self::CAPACITY],
            // Every output length is at most `CAPACITY`, which fits in `u8`.
            len: expected as u8,
        };
        digest.bytes[..expected].copy_from_slice(bytes);
        Ok(digest)
    }

    /// The digest bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl AsRef<[u8]> for Digest {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Digest(")?;
        for byte in self.as_bytes() {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

/// The bytes given for a [`Digest`] are not as long as the algorithm's
/// output.
#[derive(Debug, Snafu)]
#[snafu(display(
    "digest of {len} bytes does not match the {} byte output of {algorithm:?}",
    algorithm.output_len()
))]
pub struct DigestLengthMismatchError {
    algorithm: HashAlgorithm,
    len: usize,
}

impl DigestLengthMismatchError {
    /// The algorithm whose output length was expected.
    #[must_use]
    pub fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }

    /// The length that was given, never the output length of
    /// [`DigestLengthMismatchError::algorithm`].
    #[must_use]
    pub fn requested_len(&self) -> usize {
        self.len
    }
}

/// A hash computation in progress.
///
/// Cloning it forks the computation: the clone and the original continue
/// independently from the input both have seen.
pub trait RunningHash: Clone {
    /// Append `data` to the input.
    fn update(&mut self, data: &[u8]);

    /// The hash of everything appended so far.
    fn finish(self) -> Digest;
}

/// Hashing.
///
/// A [`Supported`] hash algorithm from [`Hash::hash_algorithm`] is also
/// accepted by the [`Hmac`](crate::Hmac) and [`Hkdf`](crate::Hkdf)
/// implementations of the same type.
pub trait Hash: Issuer {
    /// A computation started by [`Hash::start`].
    type Running: RunningHash;

    /// Whether this implementation supports `algorithm`.
    fn hash_algorithm(&self, algorithm: HashAlgorithm) -> Option<Supported<Self, HashAlgorithm>>;

    /// Start hashing with `algorithm`.
    fn start(&self, algorithm: Supported<Self, HashAlgorithm>) -> Self::Running;
}

use snafu::Snafu;

use crate::{Hash, HashAlgorithm, Secret, Supported};

/// The length of an HKDF-Expand output: 1 to 64 bytes.
///
/// HKDF itself allows up to 255 hash lengths (RFC 5869 §2.3); 64 bytes cover
/// every secret, key and IV that TLS 1.3 derives with SHA-256 or SHA-384.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExpandLength(u8);

impl ExpandLength {
    /// The shortest output.
    pub const MIN: Self = Self(1);
    /// The longest output.
    pub const MAX: Self = Self(64);

    /// An output of `len` bytes.
    ///
    /// # Errors
    ///
    /// Fails when `len` is 0 or above 64.
    pub const fn new(len: u8) -> Result<Self, ExpandLengthOutOfRange> {
        if len >= Self::MIN.0 && len <= Self::MAX.0 {
            Ok(Self(len))
        } else {
            Err(ExpandLengthOutOfRange { len })
        }
    }

    /// An output as long as the output of `algorithm`.
    #[must_use]
    pub const fn hash_len(algorithm: HashAlgorithm) -> Self {
        match algorithm {
            HashAlgorithm::Sha256 => Self(32),
            HashAlgorithm::Sha384 => Self(48),
        }
    }

    /// The length in bytes.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// An HKDF-Expand output length is 0 or above 64.
#[derive(Debug, Snafu)]
#[snafu(display(
    "hkdf output length {len} is outside {}..={}",
    ExpandLength::MIN.get(),
    ExpandLength::MAX.get()
))]
pub struct ExpandLengthOutOfRange {
    len: u8,
}

impl ExpandLengthOutOfRange {
    /// The length that was given, always 0 or above 64.
    #[must_use]
    pub fn requested_len(&self) -> u8 {
        self.len
    }
}

/// HKDF (RFC 5869) over the implementation's hash functions.
///
/// Every algorithm for which [`Hash::hash_algorithm`] issues a [`Supported`]
/// is available here. Neither step fails: the length type keeps Expand
/// within the bound of RFC 5869 §2.3.
pub trait Hkdf: Hash {
    /// HKDF-Extract: the pseudorandom key derived from `salt` and `ikm`.
    fn extract(&self, algorithm: Supported<Self, HashAlgorithm>, salt: &[u8], ikm: &[u8])
    -> Secret;

    /// HKDF-Expand: `len` bytes derived from `prk`, with the concatenation
    /// of `info` as the context.
    fn expand(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        prk: &Secret,
        info: &[&[u8]],
        len: ExpandLength,
    ) -> Secret;
}

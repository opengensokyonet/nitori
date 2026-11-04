use core::fmt;

use snafu::Snafu;
use zeroize::Zeroize;

use crate::ExpandLength;

/// Secret bytes, overwritten with zeros when dropped.
///
/// A `Secret` holds a key exchange's shared secret, an HKDF output, or any
/// other secret of at most [`Secret::CAPACITY`] bytes, inline. It is neither
/// `Clone` nor `Copy`, and `Debug` does not print the bytes.
pub struct Secret {
    bytes: [u8; Secret::CAPACITY],
    len: u8,
}

impl Secret {
    /// The longest secret a `Secret` holds: the 66-byte shared secret of
    /// ECDH over P-521.
    pub const CAPACITY: usize = 66;

    /// Copy `bytes` into a new secret.
    ///
    /// # Errors
    ///
    /// Fails when `bytes` is longer than [`Secret::CAPACITY`].
    pub fn new(bytes: &[u8]) -> Result<Self, SecretTooLong> {
        Self::from_parts(&[bytes])
    }

    /// Concatenate `parts` into a new secret.
    ///
    /// # Errors
    ///
    /// Fails when the parts together are longer than [`Secret::CAPACITY`].
    pub fn from_parts(parts: &[&[u8]]) -> Result<Self, SecretTooLong> {
        let len: usize = parts.iter().map(|part| part.len()).sum();
        if len > Self::CAPACITY {
            return SecretTooLongSnafu { len }.fail();
        }
        let mut secret = Self {
            bytes: [0; Self::CAPACITY],
            // The check above bounds `len` by `CAPACITY`, which fits in `u8`.
            len: len as u8,
        };
        let mut offset = 0;
        for part in parts {
            secret.bytes[offset..offset + part.len()].copy_from_slice(part);
            offset += part.len();
        }
        Ok(secret)
    }

    /// A secret of `len` zero bytes, to be filled through
    /// [`Secret::as_mut_bytes`].
    #[must_use]
    pub fn zeroed(len: ExpandLength) -> Self {
        Self {
            bytes: [0; Self::CAPACITY],
            len: len.get(),
        }
    }

    /// The secret bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// The secret bytes, for writing in place; the length does not change.
    #[must_use]
    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        &mut self.bytes[..usize::from(self.len)]
    }

    /// The number of secret bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    /// Whether the secret has no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secret")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

/// The bytes given for a [`Secret`] are longer than [`Secret::CAPACITY`].
#[derive(Debug, Snafu)]
#[snafu(display("secret of {len} bytes exceeds {} bytes", Secret::CAPACITY))]
pub struct SecretTooLong {
    len: usize,
}

impl SecretTooLong {
    /// The length that was given, always above [`Secret::CAPACITY`].
    #[must_use]
    pub fn requested_len(&self) -> usize {
        self.len
    }
}

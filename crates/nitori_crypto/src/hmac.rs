use snafu::Snafu;

use crate::{Digest, Hash, HashAlgorithm, Supported};

/// HMAC (RFC 2104) over the implementation's hash functions.
///
/// Every algorithm for which [`Hash::hash_algorithm`] issues a [`Supported`]
/// is available here.
pub trait Hmac: Hash {
    /// The HMAC of `message` under `key`.
    fn sign(&self, algorithm: Supported<Self, HashAlgorithm>, key: &[u8], message: &[u8])
    -> Digest;

    /// Check that `tag` is the HMAC of `message` under `key`.
    ///
    /// The comparison takes the same time wherever the tags differ.
    ///
    /// # Errors
    ///
    /// Fails when `tag` is not the HMAC, including when its length differs
    /// from the hash output length.
    fn verify(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        key: &[u8],
        message: &[u8],
        tag: &[u8],
    ) -> Result<(), TagMismatch>;
}

/// A tag is not the HMAC of the message.
#[derive(Debug, Snafu)]
#[snafu(display("hmac tag does not match"))]
pub struct TagMismatch;

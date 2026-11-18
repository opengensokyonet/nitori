//! The message_hash message (RFC 9846 §4.1).
//!
//! After a HelloRetryRequest the transcript replaces the first ClientHello
//! with a message_hash message whose body is the hash of that ClientHello.
//! The message never travels; it exists so that a server can keep only the
//! hash, for instance in a cookie.

use nitori_crypto::Digest;

/// A message_hash message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MessageHash {
    hash: Digest,
}

impl MessageHash {
    /// The message that stands for a ClientHello whose hash is `hash`.
    pub const fn new(hash: Digest) -> Self {
        Self { hash }
    }

    /// The hash of the ClientHello.
    pub const fn hash(&self) -> &Digest {
        &self.hash
    }

    /// The number of bytes [`codec::write_message_hash`] writes, the message
    /// header included.
    pub fn encoded_len(&self) -> usize {
        4 + self.hash.as_bytes().len()
    }
}

pub mod codec {
    //! Reading and writing message_hash messages.

    use super::MessageHash;
    use crate::{
        alert::AlertDescription,
        message::{HandshakeType, ReadHeaderError, read_body},
        vector::write_header,
    };
    use bytes::{Buf, BufMut};
    use nitori_crypto::{Digest, DigestLengthMismatchError, HashAlgorithm};
    use nitori_io::SplitTo;
    use snafu::{ResultExt, Snafu};

    /// Reads a message_hash message, header included, whose body is a hash
    /// of `hash`.
    ///
    /// # Errors
    ///
    /// [`ReadMessageHashError`].
    pub fn read_message_hash<S: Buf + SplitTo + AsRef<[u8]>>(
        src: &mut S,
        hash: HashAlgorithm,
    ) -> Result<MessageHash, ReadMessageHashError> {
        let body = read_body(src, HandshakeType::MESSAGE_HASH)?;
        let hash = Digest::new(hash, body.as_ref()).context(read_message_hash_error::HashSnafu)?;
        Ok(MessageHash { hash })
    }

    /// Failures that [`read_message_hash`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_message_hash_error), visibility(pub(crate)))]
    pub enum ReadMessageHashError {
        /// The header could not be read, or the message is of another type.
        #[snafu(transparent)]
        Header {
            /// The failure.
            source: ReadHeaderError,
        },
        /// The body is not as long as the hash.
        #[snafu(display("message_hash body has the wrong length"))]
        Hash {
            /// The lengths.
            source: DigestLengthMismatchError,
        },
    }

    impl ReadMessageHashError {
        /// The alert the failure calls for: unexpected_message for a message
        /// of another type, decode_error otherwise.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::Header { source } => source.alert(),
                Self::Hash { .. } => AlertDescription::DECODE_ERROR,
            }
        }
    }

    /// Writes `message`, header included.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_message_hash(dst: &mut impl BufMut, message: &MessageHash) {
        let hash = message.hash.as_bytes();
        write_header(dst, HandshakeType::MESSAGE_HASH, hash.len());
        dst.put_slice(hash)
    }
}

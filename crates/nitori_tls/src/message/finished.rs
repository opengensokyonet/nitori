//! Finished (RFC 9846 §4.5.3).
//!
//! The body of a Finished message is its verify_data, as long as the hash of
//! the cipher suite, so the reader takes that hash.

use nitori_crypto::Digest;

/// A Finished message: an HMAC over the transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Finished {
    verify_data: Digest,
}

impl Finished {
    /// The message with `verify_data`.
    pub const fn new(verify_data: Digest) -> Self {
        Self { verify_data }
    }

    /// The verify_data.
    pub const fn verify_data(&self) -> &Digest {
        &self.verify_data
    }

    /// The number of bytes [`codec::write_finished`] writes, the message
    /// header included.
    pub fn encoded_len(&self) -> usize {
        4 + self.verify_data.as_bytes().len()
    }
}

pub mod codec {
    //! Reading and writing Finished messages.

    use super::Finished;
    use crate::{
        alert::AlertDescription,
        message::{HandshakeType, ReadHeaderError, read_body},
        vector::write_header,
    };
    use bytes::{Buf, BufMut};
    use nitori_crypto::{Digest, DigestLengthMismatchError, HashAlgorithm};
    use nitori_io::SplitTo;
    use snafu::{ResultExt, Snafu};

    /// Reads a Finished message, header included, whose verify_data is as
    /// long as the output of `hash`.
    ///
    /// # Errors
    ///
    /// [`ReadFinishedError`].
    pub fn read_finished<S: Buf + SplitTo + AsRef<[u8]>>(
        src: &mut S,
        hash: HashAlgorithm,
    ) -> Result<Finished, ReadFinishedError> {
        let body = read_body(src, HandshakeType::FINISHED)?;
        let verify_data =
            Digest::new(hash, body.as_ref()).context(read_finished_error::VerifyDataSnafu)?;
        Ok(Finished { verify_data })
    }

    /// Failures that [`read_finished`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_finished_error), visibility(pub(crate)))]
    pub enum ReadFinishedError {
        /// The header could not be read, or the message is of another type.
        #[snafu(transparent)]
        Header {
            /// The failure.
            source: ReadHeaderError,
        },
        /// The body is not as long as the hash.
        #[snafu(display("Finished verify_data has the wrong length"))]
        VerifyData {
            /// The lengths.
            source: DigestLengthMismatchError,
        },
    }

    impl ReadFinishedError {
        /// The alert the failure calls for: unexpected_message for a message
        /// of another type, decode_error otherwise.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::Header { source } => source.alert(),
                Self::VerifyData { .. } => AlertDescription::DECODE_ERROR,
            }
        }
    }

    /// Writes `finished`, header included.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_finished(dst: &mut impl BufMut, finished: &Finished) {
        let verify_data = finished.verify_data.as_bytes();
        write_header(dst, HandshakeType::FINISHED, verify_data.len());
        dst.put_slice(verify_data)
    }
}

#[cfg(test)]
mod tests {
    use super::codec::{ReadFinishedError, read_finished};
    use crate::{alert::AlertDescription, test_support::message};
    use nitori_crypto::HashAlgorithm;

    #[test]
    fn verify_data_is_as_long_as_the_hash() {
        let bytes = message(20, &[0; 32]);
        let error = read_finished(&mut bytes.as_slice(), HashAlgorithm::Sha384).unwrap_err();
        let ReadFinishedError::VerifyData { source } = &error else {
            panic!("{error:?}")
        };
        assert_eq!(source.requested_len(), 32);
        assert_eq!(error.alert(), AlertDescription::DECODE_ERROR);
    }
}

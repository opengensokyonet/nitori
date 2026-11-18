//! CertificateVerify (RFC 9846 §4.5.2).

use crate::{vector::LengthOutOfRangeError, width::Width};
use nitori_crypto::SignatureScheme;

/// A CertificateVerify: a signature over the transcript, with its scheme.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CertificateVerify<C> {
    algorithm: SignatureScheme,
    signature: C,
}

impl<C: AsRef<[u8]>> CertificateVerify<C> {
    /// The message with `signature`, made with `algorithm`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] for a signature above 65535 bytes.
    pub fn new(algorithm: SignatureScheme, signature: C) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(signature.as_ref().len(), 0..=Width::U16.max())?;
        Ok(Self {
            algorithm,
            signature,
        })
    }

    fn body_len(&self) -> usize {
        2 + 2 + self.signature.as_ref().len()
    }

    /// The number of bytes [`codec::write_certificate_verify`] writes, the
    /// message header included.
    pub fn encoded_len(&self) -> usize {
        4 + self.body_len()
    }
}

impl<C> CertificateVerify<C> {
    /// The signature scheme.
    pub fn algorithm(&self) -> SignatureScheme {
        self.algorithm
    }

    /// The signature.
    pub fn signature(&self) -> &C {
        &self.signature
    }
}

pub mod codec {
    //! Reading and writing CertificateVerify messages.

    use super::CertificateVerify;
    use crate::{
        alert::AlertDescription,
        message::{HandshakeType, ReadHeaderError, read_body},
        source::read_u16,
        vector::{ExcessBytesError, ReadVectorError, read_vector, write_header, write_vector},
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_crypto::SignatureScheme;
    use nitori_io::{SplitTo, error::IncompleteError};
    use snafu::{ResultExt, Snafu};

    /// Reads a CertificateVerify message, header included.
    ///
    /// # Errors
    ///
    /// [`ReadCertificateVerifyError`].
    pub fn read_certificate_verify<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<CertificateVerify<S>, ReadCertificateVerifyError> {
        let mut body = read_body(src, HandshakeType::CERTIFICATE_VERIFY)?;
        let verify = read_certificate_verify_body(&mut body)?;
        ExcessBytesError::check(body.remaining())
            .context(read_certificate_verify_error::ExcessSnafu)?;
        Ok(verify)
    }

    fn read_certificate_verify_body<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<CertificateVerify<S>, ReadCertificateVerifyError> {
        use read_certificate_verify_error::*;
        let algorithm = SignatureScheme::new(read_u16(src).context(AlgorithmSnafu)?);
        let signature = read_vector(src, Width::U16).context(SignatureSnafu)?;
        Ok(CertificateVerify {
            algorithm,
            signature,
        })
    }

    /// Failures that [`read_certificate_verify`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_certificate_verify_error), visibility(pub(crate)))]
    pub enum ReadCertificateVerifyError {
        /// The header could not be read, or the message is of another type.
        #[snafu(transparent)]
        Header {
            /// The failure.
            source: ReadHeaderError,
        },
        /// The body is longer than its fields.
        #[snafu(display("CertificateVerify body has excess bytes"))]
        Excess {
            /// The excess.
            source: ExcessBytesError,
        },
        /// algorithm could not be read.
        #[snafu(display("CertificateVerify algorithm read failed"))]
        Algorithm {
            /// The shortfall.
            source: IncompleteError,
        },
        /// signature could not be read.
        #[snafu(display("CertificateVerify signature read failed"))]
        Signature {
            /// The failure.
            source: ReadVectorError,
        },
    }

    impl ReadCertificateVerifyError {
        /// The alert the failure calls for: unexpected_message for a message
        /// of another type, decode_error otherwise.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::Header { source } => source.alert(),
                Self::Excess { .. } | Self::Algorithm { .. } | Self::Signature { .. } => {
                    AlertDescription::DECODE_ERROR
                }
            }
        }
    }

    /// Writes `verify`, header included.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_certificate_verify<C: AsRef<[u8]>>(
        dst: &mut impl BufMut,
        verify: &CertificateVerify<C>,
    ) {
        write_header(dst, HandshakeType::CERTIFICATE_VERIFY, verify.body_len());
        dst.put_u16(verify.algorithm.get());
        write_vector(dst, Width::U16, verify.signature.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::codec::{ReadCertificateVerifyError, read_certificate_verify};
    use crate::{alert::AlertDescription, test_support::message};

    #[test]
    fn the_signature_fills_the_body() {
        let short = message(15, &[0x08, 0x04, 0x00, 0x03, 1, 2]);
        let error = read_certificate_verify(&mut short.as_slice()).unwrap_err();
        assert!(matches!(
            error,
            ReadCertificateVerifyError::Signature { .. }
        ));
        assert_eq!(error.alert(), AlertDescription::DECODE_ERROR);
        let long = message(15, &[0x08, 0x04, 0x00, 0x01, 1, 2]);
        let error = read_certificate_verify(&mut long.as_slice()).unwrap_err();
        assert!(matches!(error, ReadCertificateVerifyError::Excess { .. }));
    }
}

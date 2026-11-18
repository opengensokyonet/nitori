//! CertificateRequest (RFC 9846 §4.4.2).

use crate::extension::extension_subset;
use crate::{
    code_point::CodePointList,
    extension::{
        BlockEntry, ExtensionType, InvalidExtensionBlockError, UnknownExtension, check_block,
    },
    vector::LengthOutOfRangeError,
    width::Width,
};
use nitori_crypto::SignatureScheme;
use snafu::{ResultExt, Snafu};

/// The lengths an extension block of a CertificateRequest may have.
const BLOCK: core::ops::RangeInclusive<usize> = 2..=Width::U16.max();

/// A CertificateRequest.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CertificateRequest<C> {
    certificate_request_context: C,
    extensions: Vec<CertificateRequestExtension<C>>,
}

/// An extension of a CertificateRequest.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CertificateRequestExtension<C> {
    /// `signature_algorithms`.
    SignatureAlgorithms(CodePointList<SignatureScheme>),
    /// `signature_algorithms_cert`.
    SignatureAlgorithmsCert(CodePointList<SignatureScheme>),
    /// `signed_certificate_timestamp`, a request with empty data.
    SignedCertificateTimestamp,
    /// An extension the readers do not recognize.
    Unknown(UnknownExtension<C>),
}

impl<C: AsRef<[u8]>> BlockEntry for CertificateRequestExtension<C> {
    fn extension_type(&self) -> ExtensionType {
        match self {
            Self::SignatureAlgorithms(_) => ExtensionType::SIGNATURE_ALGORITHMS,
            Self::SignatureAlgorithmsCert(_) => ExtensionType::SIGNATURE_ALGORITHMS_CERT,
            Self::SignedCertificateTimestamp => ExtensionType::SIGNED_CERTIFICATE_TIMESTAMP,
            Self::Unknown(extension) => extension.extension_type(),
        }
    }

    fn data_len(&self) -> usize {
        match self {
            Self::SignatureAlgorithms(list) | Self::SignatureAlgorithmsCert(list) => {
                list.encoded_len()
            }
            Self::SignedCertificateTimestamp => 0,
            Self::Unknown(extension) => extension.data().as_ref().len(),
        }
    }
}

impl<C: AsRef<[u8]>> CertificateRequest<C> {
    /// The request with `certificate_request_context` and `extensions`.
    ///
    /// # Errors
    ///
    /// [`InvalidCertificateRequestError::Context`] for a context above 255 bytes,
    /// and [`InvalidCertificateRequestError::Extensions`] for an extension type
    /// that appears twice or a block outside
    /// 2 to 65535 bytes.
    pub fn new(
        certificate_request_context: C,
        extensions: Vec<CertificateRequestExtension<C>>,
    ) -> Result<Self, InvalidCertificateRequestError> {
        LengthOutOfRangeError::check(
            certificate_request_context.as_ref().len(),
            0..=Width::U8.max(),
        )
        .context(invalid_certificate_request::ContextSnafu)?;
        check_block(&extensions, BLOCK).context(invalid_certificate_request::ExtensionsSnafu)?;
        Ok(Self {
            certificate_request_context,
            extensions,
        })
    }

    fn block_len(&self) -> usize {
        self.extensions
            .iter()
            .map(|extension| 4 + extension.data_len())
            .sum()
    }

    fn body_len(&self) -> usize {
        1 + self.certificate_request_context.as_ref().len() + 2 + self.block_len()
    }

    /// The number of bytes [`codec::write_certificate_request`] writes, the
    /// message header included.
    pub fn encoded_len(&self) -> usize {
        4 + self.body_len()
    }
}

impl<C> CertificateRequest<C> {
    /// The certificate_request_context field.
    pub fn certificate_request_context(&self) -> &C {
        &self.certificate_request_context
    }

    /// The extensions, in the order of the wire.
    pub fn extensions(&self) -> &[CertificateRequestExtension<C>] {
        &self.extensions
    }
}

/// Failures of [`CertificateRequest::new`].
#[derive(Debug, Snafu)]
#[snafu(module(invalid_certificate_request), visibility(pub(crate)))]
pub enum InvalidCertificateRequestError {
    /// The context is too long.
    #[snafu(display("certificate_request_context too long"))]
    Context {
        /// The broken range.
        source: LengthOutOfRangeError,
    },
    /// The extension block is invalid.
    #[snafu(display("CertificateRequest extensions invalid"))]
    Extensions {
        /// Why.
        source: InvalidExtensionBlockError,
    },
}

extension_subset! {
    /// A recognized extension that a CertificateRequest may not carry: every one but
    /// `signature_algorithms`, `signature_algorithms_cert` and `signed_certificate_timestamp`.
    CertificateRequestDisallowedExtension {
        ServerName,
        SupportedGroups,
        ApplicationLayerProtocolNegotiation,
        ClientCertificateType,
        ServerCertificateType,
        PreSharedKey,
        EarlyData,
        SupportedVersions,
        Cookie,
        PskKeyExchangeModes,
        KeyShare,
        QuicTransportParameters,
        EncryptedClientHello,
    }
}

extension_subset! {
    /// An extension a CertificateRequest may carry whose data has a structure, so that bytes can
    /// follow it: every extension it may carry.
    CertificateRequestStructuredExtension {
        SignatureAlgorithms,
        SignedCertificateTimestamp,
        SignatureAlgorithmsCert,
    }
}

pub mod codec {
    //! Reading and writing CertificateRequest messages.

    use super::{
        BLOCK, CertificateRequest, CertificateRequestDisallowedExtension,
        CertificateRequestExtension,
    };
    use crate::{
        alert::AlertDescription,
        code_point::codec::{ReadCodePointListError, read_code_point_list, write_code_point_list},
        extension::{
            BlockEntry as _, BlockReader, ExtensionNotAllowedError, ReadExtensionBlockError,
            RestrictedExtension, UnknownExtension, excess, not_allowed, read_extension_block,
        },
        message::{HandshakeType, ReadHeaderError, read_body},
        vector::{
            ExcessBytesError, LengthOutOfRangeError, ReadVectorError, read_vector, write_header,
            write_length, write_vector,
        },
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_io::SplitTo;
    use snafu::{ResultExt, Snafu};

    /// Reads a CertificateRequest message, header included.
    ///
    /// # Errors
    ///
    /// [`ReadCertificateRequestError`].
    pub fn read_certificate_request<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<CertificateRequest<S>, ReadCertificateRequestError> {
        let mut body = read_body(src, HandshakeType::CERTIFICATE_REQUEST)?;
        let request = read_certificate_request_body(&mut body)?;
        ExcessBytesError::check(body.remaining())
            .context(read_certificate_request_error::ExcessSnafu)?;
        Ok(request)
    }

    fn read_certificate_request_body<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<CertificateRequest<S>, ReadCertificateRequestError> {
        use CertificateRequestExtension as X;
        use RestrictedExtension as R;
        use read_certificate_request_error::*;
        let certificate_request_context = read_vector(src, Width::U8).context(ContextSnafu)?;
        let block = read_extension_block(src).context(ExtensionsSnafu)?;
        LengthOutOfRangeError::check(block.remaining(), BLOCK).context(ExtensionsLengthSnafu)?;
        let mut block = BlockReader::new(block);
        let mut extensions = Vec::new();
        while let Some((extension_type, mut data)) = block.next().context(ExtensionsSnafu)? {
            let Some(restricted) = extension_type.restricted() else {
                extensions.push(X::Unknown(UnknownExtension::read(extension_type, data)));
                continue;
            };
            extensions.push(match restricted {
                R::SignatureAlgorithms => X::SignatureAlgorithms(
                    read_code_point_list(&mut data).context(SignatureAlgorithmsSnafu)?,
                ),
                R::SignatureAlgorithmsCert => X::SignatureAlgorithmsCert(
                    read_code_point_list(&mut data).context(SignatureAlgorithmsCertSnafu)?,
                ),
                R::SignedCertificateTimestamp => X::SignedCertificateTimestamp,
                other => {
                    return Err(
                        not_allowed(other, CertificateRequestDisallowedExtension::of).into(),
                    );
                }
            });
            let of = super::CertificateRequestStructuredExtension::of;
            if let Some((extension, source)) = excess(restricted, data.remaining(), of) {
                return Err(ReadCertificateRequestError::ExtensionExcess { extension, source });
            }
        }
        Ok(CertificateRequest {
            certificate_request_context,
            extensions,
        })
    }

    /// Failures that [`read_certificate_request`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_certificate_request_error), visibility(pub(crate)))]
    pub enum ReadCertificateRequestError {
        /// The header could not be read, or the message is of another type.
        #[snafu(transparent)]
        Header {
            /// The failure.
            source: ReadHeaderError,
        },
        /// The body is longer than its fields.
        #[snafu(display("CertificateRequest body has excess bytes"))]
        Excess {
            /// The excess.
            source: ExcessBytesError,
        },
        /// certificate_request_context could not be read.
        #[snafu(display("certificate_request_context read failed"))]
        Context {
            /// The failure.
            source: ReadVectorError,
        },
        /// The extension block could not be read, or a type appears twice.
        #[snafu(display("CertificateRequest extensions read failed"))]
        Extensions {
            /// The failure.
            source: ReadExtensionBlockError,
        },
        /// The extension block is shorter than 2 bytes.
        #[snafu(display("CertificateRequest extension block length out of range"))]
        ExtensionsLength {
            /// The broken range.
            source: LengthOutOfRangeError,
        },
        /// A recognized extension that this message may not carry.
        #[snafu(transparent)]
        ExtensionNotAllowed {
            /// The extension.
            source: ExtensionNotAllowedError<CertificateRequestDisallowedExtension>,
        },
        /// The data of a recognized extension is longer than its fields.
        #[snafu(display("{extension:?} data has excess bytes"))]
        ExtensionExcess {
            /// The extension.
            extension: crate::message::certificate_request::CertificateRequestStructuredExtension,
            /// The excess.
            source: ExcessBytesError,
        },
        /// `signature_algorithms` could not be read.
        #[snafu(display("signature_algorithms read failed"))]
        SignatureAlgorithms {
            /// The failure.
            source: ReadCodePointListError,
        },
        /// `signature_algorithms_cert` could not be read.
        #[snafu(display("signature_algorithms_cert read failed"))]
        SignatureAlgorithmsCert {
            /// The failure.
            source: ReadCodePointListError,
        },
    }

    impl ReadCertificateRequestError {
        /// The alert the failure calls for: illegal_parameter for a type that
        /// appears twice and for a recognized extension this message may not
        /// carry, decode_error for every failure to decode.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::Header { source } => source.alert(),
                Self::Extensions { source } => source.alert(),
                Self::ExtensionNotAllowed { source } => source.alert(),
                Self::Excess { .. }
                | Self::Context { .. }
                | Self::ExtensionsLength { .. }
                | Self::ExtensionExcess { .. }
                | Self::SignatureAlgorithms { .. }
                | Self::SignatureAlgorithmsCert { .. } => AlertDescription::DECODE_ERROR,
            }
        }
    }

    /// Writes `request`, header included.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_certificate_request<C: AsRef<[u8]>>(
        dst: &mut impl BufMut,
        request: &CertificateRequest<C>,
    ) {
        use CertificateRequestExtension as X;
        write_header(dst, HandshakeType::CERTIFICATE_REQUEST, request.body_len());
        write_vector(dst, Width::U8, request.certificate_request_context.as_ref());
        write_length(dst, Width::U16, request.block_len());
        for extension in &request.extensions {
            dst.put_u16(extension.extension_type().get());
            write_length(dst, Width::U16, extension.data_len());
            match extension {
                X::SignatureAlgorithms(list) | X::SignatureAlgorithmsCert(list) => {
                    write_code_point_list(dst, list)
                }
                X::SignedCertificateTimestamp => {}
                X::Unknown(extension) => dst.put_slice(extension.data().as_ref()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::codec::{ReadCertificateRequestError, read_certificate_request};
    use crate::{
        alert::AlertDescription,
        test_support::{concat, extension, message, vector},
    };

    fn read(extensions: &[u8]) -> Result<(), ReadCertificateRequestError> {
        let bytes = message(13, &concat(&[&[0], &vector(2, extensions)]));
        read_certificate_request(&mut bytes.as_slice()).map(|_| ())
    }

    #[test]
    fn request_rules() {
        let algorithms = extension(13, &[0x00, 0x02, 0x08, 0x04]);
        read(&algorithms).unwrap();
        let error = read(&[]).unwrap_err();
        assert!(matches!(
            error,
            ReadCertificateRequestError::ExtensionsLength { .. }
        ));
        let error = read(&concat(&[&algorithms, &extension(51, &[])])).unwrap_err();
        assert!(matches!(
            error,
            ReadCertificateRequestError::ExtensionNotAllowed { .. }
        ));
        assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
    }
}

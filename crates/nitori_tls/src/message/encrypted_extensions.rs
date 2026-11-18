//! EncryptedExtensions (RFC 9846 §4.4.1).

use crate::extension::{OpaqueData, extension_subset};
use crate::{
    code_point::CodePointList,
    extension::{
        BlockEntry, ExtensionType, InvalidExtensionBlockError, UnknownExtension,
        alpn::ProtocolName, certificate_type::CertificateType, check_block,
    },
    width::Width,
};
use nitori_crypto::NamedGroup;

/// An EncryptedExtensions message: the extensions of the server that are
/// not needed to establish the cryptographic context.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EncryptedExtensions<C> {
    extensions: Vec<EncryptedExtensionsExtension<C>>,
}

/// An extension of EncryptedExtensions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EncryptedExtensionsExtension<C> {
    /// `server_name`, with empty data: the server used the name.
    ServerName,
    /// `supported_groups`: the groups the server supports.
    SupportedGroups(CodePointList<NamedGroup>),
    /// `application_layer_protocol_negotiation`: the selected protocol.
    ApplicationLayerProtocolNegotiation(ProtocolName<C>),
    /// `client_certificate_type`: the selected type.
    ClientCertificateType(CertificateType),
    /// `server_certificate_type`: the selected type.
    ServerCertificateType(CertificateType),
    /// `early_data`, with empty data: the server accepts 0-RTT.
    EarlyData,
    /// `quic_transport_parameters`, as opaque bytes.
    QuicTransportParameters(OpaqueData<C>),
    /// `encrypted_client_hello`: the retry configurations, as opaque bytes.
    EncryptedClientHello(OpaqueData<C>),
    /// An extension the readers do not recognize.
    Unknown(UnknownExtension<C>),
}

impl<C: AsRef<[u8]>> BlockEntry for EncryptedExtensionsExtension<C> {
    fn extension_type(&self) -> ExtensionType {
        match self {
            Self::ServerName => ExtensionType::SERVER_NAME,
            Self::SupportedGroups(_) => ExtensionType::SUPPORTED_GROUPS,
            Self::ApplicationLayerProtocolNegotiation(_) => {
                ExtensionType::APPLICATION_LAYER_PROTOCOL_NEGOTIATION
            }
            Self::ClientCertificateType(_) => ExtensionType::CLIENT_CERTIFICATE_TYPE,
            Self::ServerCertificateType(_) => ExtensionType::SERVER_CERTIFICATE_TYPE,
            Self::EarlyData => ExtensionType::EARLY_DATA,
            Self::QuicTransportParameters(_) => ExtensionType::QUIC_TRANSPORT_PARAMETERS,
            Self::EncryptedClientHello(_) => ExtensionType::ENCRYPTED_CLIENT_HELLO,
            Self::Unknown(extension) => extension.extension_type(),
        }
    }

    fn data_len(&self) -> usize {
        match self {
            Self::ServerName | Self::EarlyData => 0,
            Self::SupportedGroups(list) => list.encoded_len(),
            Self::ApplicationLayerProtocolNegotiation(name) => name.encoded_len(),
            Self::ClientCertificateType(_) | Self::ServerCertificateType(_) => 1,
            Self::QuicTransportParameters(data) | Self::EncryptedClientHello(data) => {
                data.data().as_ref().len()
            }
            Self::Unknown(extension) => extension.data().as_ref().len(),
        }
    }
}

impl<C: AsRef<[u8]>> EncryptedExtensions<C> {
    /// The message with `extensions`.
    ///
    /// # Errors
    ///
    /// [`InvalidExtensionBlockError`] for an extension type that appears twice or a block above 65535 bytes.
    pub fn new(
        extensions: Vec<EncryptedExtensionsExtension<C>>,
    ) -> Result<Self, InvalidExtensionBlockError> {
        check_block(&extensions, 0..=Width::U16.max())?;
        Ok(Self { extensions })
    }

    fn block_len(&self) -> usize {
        self.extensions
            .iter()
            .map(|extension| 4 + extension.data_len())
            .sum()
    }

    /// The number of bytes [`codec::write_encrypted_extensions`] writes, the
    /// message header included.
    pub fn encoded_len(&self) -> usize {
        4 + 2 + self.block_len()
    }
}

impl<C> EncryptedExtensions<C> {
    /// The extensions, in the order of the wire.
    pub fn extensions(&self) -> &[EncryptedExtensionsExtension<C>] {
        &self.extensions
    }

    /// The transport parameters, if present.
    pub fn quic_transport_parameters(&self) -> Option<&C> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                EncryptedExtensionsExtension::QuicTransportParameters(data) => Some(data.data()),
                _ => None,
            })
    }
}

extension_subset! {
    /// A recognized extension that EncryptedExtensions may not carry: every one but `server_name`,
    /// `supported_groups`, `application_layer_protocol_negotiation`, `client_certificate_type`,
    /// `server_certificate_type`, `early_data`, `quic_transport_parameters` and
    /// `encrypted_client_hello`.
    EncryptedExtensionsDisallowedExtension {
        SignatureAlgorithms,
        SignedCertificateTimestamp,
        PreSharedKey,
        SupportedVersions,
        Cookie,
        PskKeyExchangeModes,
        SignatureAlgorithmsCert,
        KeyShare,
    }
}

extension_subset! {
    /// An extension EncryptedExtensions may carry whose data has a structure, so that bytes can
    /// follow it: every extension it may carry but `quic_transport_parameters` and
    /// `encrypted_client_hello`, whose data is kept whole.
    EncryptedExtensionsStructuredExtension {
        ServerName,
        SupportedGroups,
        ApplicationLayerProtocolNegotiation,
        ClientCertificateType,
        ServerCertificateType,
        EarlyData,
    }
}

pub mod codec {
    //! Reading and writing EncryptedExtensions messages.

    use super::{
        EncryptedExtensions, EncryptedExtensionsDisallowedExtension, EncryptedExtensionsExtension,
    };
    use crate::{
        alert::AlertDescription,
        code_point::codec::{ReadCodePointListError, read_code_point_list, write_code_point_list},
        extension::{
            BlockEntry as _, BlockReader, ExtensionNotAllowedError, ReadExtensionBlockError,
            RestrictedExtension, UnknownExtension,
            alpn::codec::{ReadSelectedProtocolError, read_selected_protocol},
            certificate_type::CertificateType,
            excess, not_allowed, read_extension_block,
        },
        message::{HandshakeType, ReadHeaderError, read_body},
        source::{read_u8, take_rest},
        vector::{ExcessBytesError, write_header, write_length},
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_io::{SplitTo, error::IncompleteError};
    use snafu::{ResultExt, Snafu};

    /// Reads an EncryptedExtensions message, header included.
    ///
    /// # Errors
    ///
    /// [`ReadEncryptedExtensionsError`].
    pub fn read_encrypted_extensions<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<EncryptedExtensions<S>, ReadEncryptedExtensionsError> {
        use read_encrypted_extensions_error::*;
        let mut body = read_body(src, HandshakeType::ENCRYPTED_EXTENSIONS)?;
        let block = read_extension_block(&mut body).context(ExtensionsSnafu)?;
        ExcessBytesError::check(body.remaining()).context(ExcessSnafu)?;
        let mut block = BlockReader::new(block);
        let mut extensions = Vec::new();
        while let Some((extension_type, mut data)) = block.next().context(ExtensionsSnafu)? {
            let Some(restricted) = extension_type.restricted() else {
                let extension = UnknownExtension::read(extension_type, data);
                extensions.push(EncryptedExtensionsExtension::Unknown(extension));
                continue;
            };
            extensions.push(read_extension(restricted, &mut data)?);
            let of = super::EncryptedExtensionsStructuredExtension::of;
            if let Some((extension, source)) = excess(restricted, data.remaining(), of) {
                return Err(ReadEncryptedExtensionsError::ExtensionExcess { extension, source });
            }
        }
        Ok(EncryptedExtensions { extensions })
    }

    /// Reads the data of a recognized extension.
    fn read_extension<B>(
        extension: RestrictedExtension,
        data: &mut B,
    ) -> Result<EncryptedExtensionsExtension<B>, ReadEncryptedExtensionsError>
    where
        B: Buf + SplitTo,
    {
        use EncryptedExtensionsExtension as X;
        use RestrictedExtension as R;
        use read_encrypted_extensions_error::*;
        Ok(match extension {
            R::ServerName => X::ServerName,
            R::SupportedGroups => {
                X::SupportedGroups(read_code_point_list(data).context(SupportedGroupsSnafu)?)
            }
            R::ApplicationLayerProtocolNegotiation => X::ApplicationLayerProtocolNegotiation(
                read_selected_protocol(data).context(ApplicationLayerProtocolNegotiationSnafu)?,
            ),
            R::ClientCertificateType => X::ClientCertificateType(CertificateType::new(
                read_u8(data).context(ClientCertificateTypeSnafu)?,
            )),
            R::ServerCertificateType => X::ServerCertificateType(CertificateType::new(
                read_u8(data).context(ServerCertificateTypeSnafu)?,
            )),
            R::EarlyData => X::EarlyData,
            R::QuicTransportParameters => {
                X::QuicTransportParameters(crate::extension::OpaqueData::read(take_rest(data)))
            }
            R::EncryptedClientHello => {
                X::EncryptedClientHello(crate::extension::OpaqueData::read(take_rest(data)))
            }
            other => {
                return Err(not_allowed(other, EncryptedExtensionsDisallowedExtension::of).into());
            }
        })
    }

    /// Failures that [`read_encrypted_extensions`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_encrypted_extensions_error), visibility(pub(crate)))]
    pub enum ReadEncryptedExtensionsError {
        /// The header could not be read, or the message is of another type.
        #[snafu(transparent)]
        Header {
            /// The failure.
            source: ReadHeaderError,
        },
        /// The body is longer than the extension block.
        #[snafu(display("EncryptedExtensions body has excess bytes"))]
        Excess {
            /// The excess.
            source: ExcessBytesError,
        },
        /// The extension block could not be read, or a type appears twice.
        #[snafu(display("EncryptedExtensions extensions read failed"))]
        Extensions {
            /// The failure.
            source: ReadExtensionBlockError,
        },
        /// A recognized extension that this message may not carry.
        #[snafu(transparent)]
        ExtensionNotAllowed {
            /// The extension.
            source: ExtensionNotAllowedError<EncryptedExtensionsDisallowedExtension>,
        },
        /// The data of a recognized extension is longer than its fields.
        #[snafu(display("{extension:?} data has excess bytes"))]
        ExtensionExcess {
            /// The extension.
            extension: crate::message::encrypted_extensions::EncryptedExtensionsStructuredExtension,
            /// The excess.
            source: ExcessBytesError,
        },
        /// `supported_groups` could not be read.
        #[snafu(display("supported_groups read failed"))]
        SupportedGroups {
            /// The failure.
            source: ReadCodePointListError,
        },
        /// `application_layer_protocol_negotiation` could not be read or
        /// does not hold exactly one name.
        #[snafu(display("application_layer_protocol_negotiation read failed"))]
        ApplicationLayerProtocolNegotiation {
            /// The failure.
            source: ReadSelectedProtocolError,
        },
        /// `client_certificate_type` could not be read.
        #[snafu(display("client_certificate_type read failed"))]
        ClientCertificateType {
            /// The shortfall.
            source: IncompleteError,
        },
        /// `server_certificate_type` could not be read.
        #[snafu(display("server_certificate_type read failed"))]
        ServerCertificateType {
            /// The shortfall.
            source: IncompleteError,
        },
    }

    impl ReadEncryptedExtensionsError {
        /// The alert the failure calls for.
        ///
        /// illegal_parameter for a type that appears twice, for a recognized
        /// extension this message may not carry (RFC 9846 §4.4.1) and for a
        /// protocol list that does not hold exactly one name; decode_error
        /// for every failure to decode.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::Header { source } => source.alert(),
                Self::Extensions { source } => source.alert(),
                Self::ExtensionNotAllowed { source } => source.alert(),
                Self::ApplicationLayerProtocolNegotiation { source } => source.alert(),
                Self::Excess { .. }
                | Self::ExtensionExcess { .. }
                | Self::SupportedGroups { .. }
                | Self::ClientCertificateType { .. }
                | Self::ServerCertificateType { .. } => AlertDescription::DECODE_ERROR,
            }
        }
    }

    /// Writes `message`, header included.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_encrypted_extensions<C: AsRef<[u8]>>(
        dst: &mut impl BufMut,
        message: &EncryptedExtensions<C>,
    ) {
        use EncryptedExtensionsExtension as X;
        let block_len = message.block_len();
        write_header(dst, HandshakeType::ENCRYPTED_EXTENSIONS, 2 + block_len);
        write_length(dst, Width::U16, block_len);
        for extension in &message.extensions {
            dst.put_u16(extension.extension_type().get());
            write_length(dst, Width::U16, extension.data_len());
            match extension {
                X::ServerName | X::EarlyData => {}
                X::SupportedGroups(list) => write_code_point_list(dst, list),
                X::ApplicationLayerProtocolNegotiation(name) => name.encode_selected(dst),
                X::ClientCertificateType(kind) | X::ServerCertificateType(kind) => {
                    dst.put_u8(kind.get())
                }
                X::QuicTransportParameters(data) | X::EncryptedClientHello(data) => {
                    dst.put_slice(data.data().as_ref())
                }
                X::Unknown(extension) => dst.put_slice(extension.data().as_ref()),
            }
        }
    }
}

#[cfg(test)]
mod tests;

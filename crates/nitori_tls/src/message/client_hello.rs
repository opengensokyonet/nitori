//! ClientHello (RFC 9846 §4.2.2).
//!
//! A ClientHello is read whole: the transcript hashes its exact bytes, a PSK
//! binder covers the bytes before the binders, and the extension block is
//! checked as a whole, so that no type appears twice and `pre_shared_key`
//! is the last extension (RFC 9846 §4.3, §4.3.11).

use crate::extension::{OpaqueData, extension_subset};
use crate::{
    cipher_suite::CipherSuite,
    code_point::{ByteCodePointList, CodePointList},
    extension::{
        BlockEntry, ExtensionType, InvalidExtensionBlockError, UnknownExtension,
        alpn::ProtocolNameList, certificate_type::CertificateType, check_block, cookie::Cookie,
        key_share::ClientShares, pre_shared_key::OfferedPsks,
        psk_key_exchange_modes::PskKeyExchangeMode, server_name::ServerName,
    },
    message::{HelloRandom, LegacySessionId},
    version::ProtocolVersion,
    width::Width,
};
use nitori_crypto::{NamedGroup, SignatureScheme};
use snafu::{ResultExt, Snafu};

/// A ClientHello.
///
/// `legacy_compression_methods` is always the single null method, so the
/// value does not hold it. The extensions keep their order on the wire.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClientHello<C> {
    legacy_version: ProtocolVersion,
    random: HelloRandom,
    legacy_session_id: LegacySessionId,
    cipher_suites: CodePointList<CipherSuite>,
    extensions: Vec<ClientHelloExtension<C>>,
}

/// An extension of a ClientHello. Every recognized extension may appear in
/// a ClientHello.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ClientHelloExtension<C> {
    /// `server_name`.
    ServerName(ServerName<C>),
    /// `supported_groups`.
    SupportedGroups(CodePointList<NamedGroup>),
    /// `signature_algorithms`.
    SignatureAlgorithms(CodePointList<SignatureScheme>),
    /// `application_layer_protocol_negotiation`.
    ApplicationLayerProtocolNegotiation(ProtocolNameList<C>),
    /// `signed_certificate_timestamp`, a request with empty data.
    SignedCertificateTimestamp,
    /// `client_certificate_type`.
    ClientCertificateType(ByteCodePointList<CertificateType>),
    /// `server_certificate_type`.
    ServerCertificateType(ByteCodePointList<CertificateType>),
    /// `pre_shared_key`.
    PreSharedKey(OfferedPsks<C>),
    /// `early_data`, with empty data.
    EarlyData,
    /// `supported_versions`.
    SupportedVersions(CodePointList<ProtocolVersion>),
    /// `cookie`.
    Cookie(Cookie<C>),
    /// `psk_key_exchange_modes`.
    PskKeyExchangeModes(ByteCodePointList<PskKeyExchangeMode>),
    /// `signature_algorithms_cert`.
    SignatureAlgorithmsCert(CodePointList<SignatureScheme>),
    /// `key_share`.
    KeyShare(ClientShares<C>),
    /// `quic_transport_parameters`, as opaque bytes.
    QuicTransportParameters(OpaqueData<C>),
    /// `encrypted_client_hello`, as opaque bytes.
    EncryptedClientHello(OpaqueData<C>),
    /// An extension the readers do not recognize.
    Unknown(UnknownExtension<C>),
}

impl<C> ClientHelloExtension<C> {
    /// The extension type.
    pub fn extension_type(&self) -> ExtensionType {
        match self {
            Self::ServerName(_) => ExtensionType::SERVER_NAME,
            Self::SupportedGroups(_) => ExtensionType::SUPPORTED_GROUPS,
            Self::SignatureAlgorithms(_) => ExtensionType::SIGNATURE_ALGORITHMS,
            Self::ApplicationLayerProtocolNegotiation(_) => {
                ExtensionType::APPLICATION_LAYER_PROTOCOL_NEGOTIATION
            }
            Self::SignedCertificateTimestamp => ExtensionType::SIGNED_CERTIFICATE_TIMESTAMP,
            Self::ClientCertificateType(_) => ExtensionType::CLIENT_CERTIFICATE_TYPE,
            Self::ServerCertificateType(_) => ExtensionType::SERVER_CERTIFICATE_TYPE,
            Self::PreSharedKey(_) => ExtensionType::PRE_SHARED_KEY,
            Self::EarlyData => ExtensionType::EARLY_DATA,
            Self::SupportedVersions(_) => ExtensionType::SUPPORTED_VERSIONS,
            Self::Cookie(_) => ExtensionType::COOKIE,
            Self::PskKeyExchangeModes(_) => ExtensionType::PSK_KEY_EXCHANGE_MODES,
            Self::SignatureAlgorithmsCert(_) => ExtensionType::SIGNATURE_ALGORITHMS_CERT,
            Self::KeyShare(_) => ExtensionType::KEY_SHARE,
            Self::QuicTransportParameters(_) => ExtensionType::QUIC_TRANSPORT_PARAMETERS,
            Self::EncryptedClientHello(_) => ExtensionType::ENCRYPTED_CLIENT_HELLO,
            Self::Unknown(extension) => extension.extension_type(),
        }
    }
}

impl<C: AsRef<[u8]>> BlockEntry for ClientHelloExtension<C> {
    fn extension_type(&self) -> ExtensionType {
        ClientHelloExtension::extension_type(self)
    }

    fn data_len(&self) -> usize {
        match self {
            Self::ServerName(name) => name.encoded_len(),
            Self::SupportedGroups(list) => list.encoded_len(),
            Self::SignatureAlgorithms(list) | Self::SignatureAlgorithmsCert(list) => {
                list.encoded_len()
            }
            Self::ApplicationLayerProtocolNegotiation(list) => list.encoded_len(),
            Self::SignedCertificateTimestamp | Self::EarlyData => 0,
            Self::ClientCertificateType(list) | Self::ServerCertificateType(list) => {
                list.encoded_len()
            }
            Self::PreSharedKey(offered) => offered.encoded_len(),
            Self::SupportedVersions(list) => list.encoded_len(),
            Self::Cookie(cookie) => cookie.encoded_len(),
            Self::PskKeyExchangeModes(list) => list.encoded_len(),
            Self::KeyShare(shares) => shares.encoded_len(),
            Self::QuicTransportParameters(data) | Self::EncryptedClientHello(data) => {
                data.data().as_ref().len()
            }
            Self::Unknown(extension) => extension.data().as_ref().len(),
        }
    }
}

/// The lengths an extension block of a ClientHello may have.
const BLOCK: core::ops::RangeInclusive<usize> = 8..=Width::U16.max();

impl<C: AsRef<[u8]>> ClientHello<C> {
    /// The ClientHello with these fields.
    ///
    /// # Errors
    ///
    /// [`InvalidClientHelloError::Extensions`] for an extension type that appears
    /// twice or a block outside 8 to 65535
    /// bytes, and [`InvalidClientHelloError::PreSharedKeyNotLast`] if
    /// `pre_shared_key` is not the last extension.
    pub fn new(
        legacy_version: ProtocolVersion,
        random: HelloRandom,
        legacy_session_id: LegacySessionId,
        cipher_suites: CodePointList<CipherSuite>,
        extensions: Vec<ClientHelloExtension<C>>,
    ) -> Result<Self, InvalidClientHelloError> {
        check_block(&extensions, BLOCK).context(invalid_client_hello::ExtensionsSnafu)?;
        if let Some(position) = extensions
            .iter()
            .position(|extension| matches!(extension, ClientHelloExtension::PreSharedKey(_)))
            && position + 1 != extensions.len()
        {
            return invalid_client_hello::PreSharedKeyNotLastSnafu.fail();
        }
        Ok(Self {
            legacy_version,
            random,
            legacy_session_id,
            cipher_suites,
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
        2 + 32
            + self.legacy_session_id.encoded_len()
            + self.cipher_suites.encoded_len()
            + 2
            + 2
            + self.block_len()
    }

    /// The number of bytes [`codec::write_client_hello`] writes, the
    /// message header included.
    pub fn encoded_len(&self) -> usize {
        4 + self.body_len()
    }

    /// The number of bytes the binders of `pre_shared_key` take at the end
    /// of the encoding, or `None` without that extension.
    ///
    /// A binder covers the encoding without these bytes
    /// (RFC 9846 §4.3.11.2).
    pub fn binders_len(&self) -> Option<usize> {
        self.pre_shared_key().map(OfferedPsks::binders_len)
    }
}

impl<C> ClientHello<C> {
    /// The legacy_version field.
    pub fn legacy_version(&self) -> ProtocolVersion {
        self.legacy_version
    }

    /// The random field.
    pub fn random(&self) -> &HelloRandom {
        &self.random
    }

    /// The legacy_session_id field.
    pub fn legacy_session_id(&self) -> &LegacySessionId {
        &self.legacy_session_id
    }

    /// The cipher suites, in the client's preference order.
    pub fn cipher_suites(&self) -> &CodePointList<CipherSuite> {
        &self.cipher_suites
    }

    /// The extensions, in the order of the wire.
    pub fn extensions(&self) -> &[ClientHelloExtension<C>] {
        &self.extensions
    }

    /// The data of `key_share`, if present.
    pub fn key_share(&self) -> Option<&ClientShares<C>> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                ClientHelloExtension::KeyShare(shares) => Some(shares),
                _ => None,
            })
    }

    /// The data of `server_name`, if present.
    pub fn server_name(&self) -> Option<&ServerName<C>> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                ClientHelloExtension::ServerName(name) => Some(name),
                _ => None,
            })
    }

    /// The data of `supported_groups`, if present.
    pub fn supported_groups(&self) -> Option<&CodePointList<NamedGroup>> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                ClientHelloExtension::SupportedGroups(groups) => Some(groups),
                _ => None,
            })
    }

    /// The data of `signature_algorithms`, if present.
    pub fn signature_algorithms(&self) -> Option<&CodePointList<SignatureScheme>> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                ClientHelloExtension::SignatureAlgorithms(schemes) => Some(schemes),
                _ => None,
            })
    }

    /// The data of `application_layer_protocol_negotiation`, if present.
    pub fn application_layer_protocol_negotiation(&self) -> Option<&ProtocolNameList<C>> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                ClientHelloExtension::ApplicationLayerProtocolNegotiation(names) => Some(names),
                _ => None,
            })
    }

    /// The data of `pre_shared_key`, if present.
    pub fn pre_shared_key(&self) -> Option<&OfferedPsks<C>> {
        match self.extensions.last() {
            Some(ClientHelloExtension::PreSharedKey(offered)) => Some(offered),
            _ => None,
        }
    }
}

/// Failures of [`ClientHello::new`].
#[derive(Debug, Snafu)]
#[snafu(module(invalid_client_hello), visibility(pub(crate)))]
pub enum InvalidClientHelloError {
    /// The extension block is invalid.
    #[snafu(display("ClientHello extensions invalid"))]
    Extensions {
        /// Why.
        source: InvalidExtensionBlockError,
    },
    /// `pre_shared_key` is not the last extension.
    #[snafu(display("pre_shared_key is not the last ClientHello extension"))]
    PreSharedKeyNotLast,
}

extension_subset! {
    /// An extension a ClientHello may carry whose data has a structure, so that bytes can follow
    /// it: every recognized extension but `quic_transport_parameters` and `encrypted_client_hello`,
    /// whose data is kept whole.
    ClientHelloStructuredExtension {
        ServerName,
        SupportedGroups,
        SignatureAlgorithms,
        ApplicationLayerProtocolNegotiation,
        SignedCertificateTimestamp,
        ClientCertificateType,
        ServerCertificateType,
        PreSharedKey,
        EarlyData,
        SupportedVersions,
        Cookie,
        PskKeyExchangeModes,
        SignatureAlgorithmsCert,
        KeyShare,
    }
}

pub mod codec {
    //! Reading and writing ClientHello messages.

    use super::{BLOCK, ClientHello, ClientHelloExtension, ClientHelloStructuredExtension};
    use crate::{
        alert::AlertDescription,
        cipher_suite::CipherSuite,
        code_point::codec::{
            ReadByteCodePointListError, ReadCodePointListError, read_code_point_list,
            write_byte_code_point_list, write_code_point_list,
        },
        extension::{
            BlockEntry as _, BlockReader, ReadExtensionBlockError, RestrictedExtension,
            UnknownExtension, alpn::codec::ReadProtocolNameListError, excess,
            key_share::codec::ReadClientSharesError, pre_shared_key::codec::ReadOfferedPsksError,
            read_extension_block, server_name::codec::ReadServerNameError,
        },
        message::{HandshakeType, ReadHeaderError, read_body, read_session_id},
        source::{read_array, read_u16, take_rest},
        vector::{
            ExcessBytesError, LengthOutOfRangeError, ReadBoundedVectorError, read_bounded_vector,
            write_header, write_length, write_vector,
        },
        version::ProtocolVersion,
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_io::{SplitTo, error::IncompleteError};
    use snafu::{ResultExt, Snafu};

    /// Reads a ClientHello message, header included.
    ///
    /// # Errors
    ///
    /// [`ReadClientHelloError`].
    pub fn read_client_hello<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ClientHello<S>, ReadClientHelloError> {
        let mut body = read_body(src, HandshakeType::CLIENT_HELLO)?;
        let hello = read_client_hello_body(&mut body)?;
        ExcessBytesError::check(body.remaining()).context(read_client_hello_error::ExcessSnafu)?;
        Ok(hello)
    }

    fn read_client_hello_body<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ClientHello<S>, ReadClientHelloError> {
        use read_client_hello_error::*;
        let legacy_version = ProtocolVersion::new(read_u16(src).context(LegacyVersionSnafu)?);
        let random = super::HelloRandom(read_array(src).context(RandomSnafu)?);
        let legacy_session_id = read_session_id(src).context(SessionIdSnafu)?;
        let cipher_suites =
            read_code_point_list::<CipherSuite, _>(src).context(CipherSuitesSnafu)?;
        let mut compression = read_bounded_vector(src, Width::U8, 1..=Width::U8.max())
            .context(CompressionMethodsSnafu)?;
        if compression.remaining() != 1 || compression.get_u8() != 0 {
            return CompressionMethodsNotNullSnafu.fail();
        }
        let block = read_extension_block(src).context(ExtensionsSnafu)?;
        LengthOutOfRangeError::check(block.remaining(), BLOCK).context(ExtensionsLengthSnafu)?;
        let mut block = BlockReader::new(block);
        let mut extensions = Vec::new();
        while let Some((extension_type, mut data)) = block.next().context(ExtensionsSnafu)? {
            if matches!(
                extensions.last(),
                Some(ClientHelloExtension::PreSharedKey(_))
            ) {
                return PreSharedKeyNotLastSnafu.fail();
            }
            let Some(restricted) = extension_type.restricted() else {
                extensions.push(ClientHelloExtension::Unknown(UnknownExtension::read(
                    extension_type,
                    data,
                )));
                continue;
            };
            let extension = read_extension(restricted, &mut data)?;
            if let Some((extension, source)) = excess(
                restricted,
                data.remaining(),
                ClientHelloStructuredExtension::of,
            ) {
                return Err(ReadClientHelloError::ExtensionExcess { extension, source });
            }
            extensions.push(extension);
        }
        Ok(ClientHello {
            legacy_version,
            random,
            legacy_session_id,
            cipher_suites,
            extensions,
        })
    }

    /// Reads the data of the recognized extension `extension` from `data`.
    fn read_extension<B>(
        extension: RestrictedExtension,
        data: &mut B,
    ) -> Result<ClientHelloExtension<B>, ReadClientHelloError>
    where
        B: Buf + SplitTo,
    {
        use crate::{
            code_point::codec::{read_byte_code_point_list, read_code_point_list},
            extension::{
                alpn::codec::read_protocol_name_list, cookie::codec::read_cookie,
                key_share::codec::read_client_shares, pre_shared_key::codec::read_offered_psks,
                server_name::codec::read_server_name,
            },
        };
        use ClientHelloExtension as X;
        use RestrictedExtension as R;
        use read_client_hello_error::*;
        Ok(match extension {
            R::ServerName => X::ServerName(read_server_name(data).context(ServerNameSnafu)?),
            R::SupportedGroups => {
                X::SupportedGroups(read_code_point_list(data).context(SupportedGroupsSnafu)?)
            }
            R::SignatureAlgorithms => X::SignatureAlgorithms(
                read_code_point_list(data).context(SignatureAlgorithmsSnafu)?,
            ),
            R::ApplicationLayerProtocolNegotiation => X::ApplicationLayerProtocolNegotiation(
                read_protocol_name_list(data).context(ApplicationLayerProtocolNegotiationSnafu)?,
            ),
            R::SignedCertificateTimestamp => X::SignedCertificateTimestamp,
            R::ClientCertificateType => X::ClientCertificateType(
                read_byte_code_point_list(data).context(ClientCertificateTypeSnafu)?,
            ),
            R::ServerCertificateType => X::ServerCertificateType(
                read_byte_code_point_list(data).context(ServerCertificateTypeSnafu)?,
            ),
            R::PreSharedKey => X::PreSharedKey(read_offered_psks(data).context(PreSharedKeySnafu)?),
            R::EarlyData => X::EarlyData,
            R::SupportedVersions => {
                X::SupportedVersions(read_code_point_list(data).context(SupportedVersionsSnafu)?)
            }
            R::Cookie => X::Cookie(read_cookie(data).context(CookieSnafu)?),
            R::PskKeyExchangeModes => X::PskKeyExchangeModes(
                read_byte_code_point_list(data).context(PskKeyExchangeModesSnafu)?,
            ),
            R::SignatureAlgorithmsCert => X::SignatureAlgorithmsCert(
                read_code_point_list(data).context(SignatureAlgorithmsCertSnafu)?,
            ),
            R::KeyShare => X::KeyShare(read_client_shares(data).context(KeyShareSnafu)?),
            R::QuicTransportParameters => {
                X::QuicTransportParameters(crate::extension::OpaqueData::read(take_rest(data)))
            }
            R::EncryptedClientHello => {
                X::EncryptedClientHello(crate::extension::OpaqueData::read(take_rest(data)))
            }
        })
    }

    /// Failures that [`read_client_hello`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_client_hello_error), visibility(pub(crate)))]
    pub enum ReadClientHelloError {
        /// The header could not be read, or the message is of another type.
        #[snafu(transparent)]
        Header {
            /// The failure.
            source: ReadHeaderError,
        },
        /// The body is longer than its fields.
        #[snafu(display("ClientHello body has excess bytes"))]
        Excess {
            /// The excess.
            source: ExcessBytesError,
        },
        /// legacy_version could not be read.
        #[snafu(display("ClientHello legacy_version read failed"))]
        LegacyVersion {
            /// The shortfall.
            source: IncompleteError,
        },
        /// random could not be read.
        #[snafu(display("ClientHello random read failed"))]
        Random {
            /// The shortfall.
            source: IncompleteError,
        },
        /// legacy_session_id could not be read or is longer than 32 bytes.
        #[snafu(display("ClientHello legacy_session_id read failed"))]
        SessionId {
            /// The failure of reading the field.
            source: ReadBoundedVectorError,
        },
        /// cipher_suites could not be read.
        #[snafu(display("ClientHello cipher_suites read failed"))]
        CipherSuites {
            /// The failure of reading the list.
            source: ReadCodePointListError,
        },
        /// legacy_compression_methods could not be read.
        #[snafu(display("ClientHello legacy_compression_methods read failed"))]
        CompressionMethods {
            /// The failure of reading the field.
            source: ReadBoundedVectorError,
        },
        /// legacy_compression_methods is not the single null method
        /// (RFC 9846 §4.2.2).
        #[snafu(display("ClientHello legacy_compression_methods is not the null method"))]
        CompressionMethodsNotNull,
        /// The extension block could not be read, or a type appears twice.
        #[snafu(display("ClientHello extensions read failed"))]
        Extensions {
            /// The failure.
            source: ReadExtensionBlockError,
        },
        /// The extension block is shorter than 8 bytes.
        #[snafu(display("ClientHello extension block length out of range"))]
        ExtensionsLength {
            /// The broken range.
            source: LengthOutOfRangeError,
        },
        /// `pre_shared_key` is not the last extension (RFC 9846 §4.3.11).
        #[snafu(display("pre_shared_key is not the last ClientHello extension"))]
        PreSharedKeyNotLast,
        /// The data of a recognized extension is longer than its fields.
        #[snafu(display("{extension:?} data has excess bytes"))]
        ExtensionExcess {
            /// The extension.
            extension: ClientHelloStructuredExtension,
            /// The excess.
            source: ExcessBytesError,
        },
        /// `server_name` could not be read.
        #[snafu(display("server_name read failed"))]
        ServerName {
            /// The failure.
            source: ReadServerNameError,
        },
        /// `supported_groups` could not be read.
        #[snafu(display("supported_groups read failed"))]
        SupportedGroups {
            /// The failure.
            source: ReadCodePointListError,
        },
        /// `signature_algorithms` could not be read.
        #[snafu(display("signature_algorithms read failed"))]
        SignatureAlgorithms {
            /// The failure.
            source: ReadCodePointListError,
        },
        /// `application_layer_protocol_negotiation` could not be read.
        #[snafu(display("application_layer_protocol_negotiation read failed"))]
        ApplicationLayerProtocolNegotiation {
            /// The failure.
            source: ReadProtocolNameListError,
        },
        /// `client_certificate_type` could not be read.
        #[snafu(display("client_certificate_type read failed"))]
        ClientCertificateType {
            /// The failure.
            source: ReadByteCodePointListError,
        },
        /// `server_certificate_type` could not be read.
        #[snafu(display("server_certificate_type read failed"))]
        ServerCertificateType {
            /// The failure.
            source: ReadByteCodePointListError,
        },
        /// `pre_shared_key` could not be read.
        #[snafu(display("pre_shared_key read failed"))]
        PreSharedKey {
            /// The failure.
            source: ReadOfferedPsksError,
        },
        /// `supported_versions` could not be read.
        #[snafu(display("supported_versions read failed"))]
        SupportedVersions {
            /// The failure.
            source: ReadCodePointListError,
        },
        /// `cookie` could not be read.
        #[snafu(display("cookie read failed"))]
        Cookie {
            /// The failure.
            source: ReadBoundedVectorError,
        },
        /// `psk_key_exchange_modes` could not be read.
        #[snafu(display("psk_key_exchange_modes read failed"))]
        PskKeyExchangeModes {
            /// The failure.
            source: ReadByteCodePointListError,
        },
        /// `signature_algorithms_cert` could not be read.
        #[snafu(display("signature_algorithms_cert read failed"))]
        SignatureAlgorithmsCert {
            /// The failure.
            source: ReadCodePointListError,
        },
        /// `key_share` could not be read.
        #[snafu(display("key_share read failed"))]
        KeyShare {
            /// The failure.
            source: ReadClientSharesError,
        },
    }

    impl ReadClientHelloError {
        /// The alert the failure calls for.
        ///
        /// illegal_parameter for compression methods other than the null
        /// method and for `pre_shared_key` before another extension
        /// (RFC 9846 §4.2.2, §4.3.11), for a type that appears twice and for
        /// the rules within an extension that name it; decode_error for every
        /// failure to decode.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::Header { source } => source.alert(),
                Self::CompressionMethodsNotNull | Self::PreSharedKeyNotLast => {
                    AlertDescription::ILLEGAL_PARAMETER
                }
                Self::Extensions { source } => source.alert(),
                Self::ServerName { source } => source.alert(),
                Self::PreSharedKey { source } => source.alert(),
                Self::KeyShare { source } => source.alert(),
                Self::Excess { .. }
                | Self::LegacyVersion { .. }
                | Self::Random { .. }
                | Self::SessionId { .. }
                | Self::CipherSuites { .. }
                | Self::CompressionMethods { .. }
                | Self::ExtensionsLength { .. }
                | Self::ExtensionExcess { .. }
                | Self::SupportedGroups { .. }
                | Self::SignatureAlgorithms { .. }
                | Self::ApplicationLayerProtocolNegotiation { .. }
                | Self::ClientCertificateType { .. }
                | Self::ServerCertificateType { .. }
                | Self::SupportedVersions { .. }
                | Self::Cookie { .. }
                | Self::PskKeyExchangeModes { .. }
                | Self::SignatureAlgorithmsCert { .. } => AlertDescription::DECODE_ERROR,
            }
        }
    }

    /// Writes `hello`, header included.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_client_hello<C: AsRef<[u8]>>(dst: &mut impl BufMut, hello: &ClientHello<C>) {
        write_header(dst, HandshakeType::CLIENT_HELLO, hello.body_len());
        dst.put_u16(hello.legacy_version.get());
        dst.put_slice(&hello.random.0[..]);
        write_vector(dst, Width::U8, hello.legacy_session_id.as_bytes());
        write_code_point_list(dst, &hello.cipher_suites);
        dst.put_slice(&[1, 0][..]);
        write_length(dst, Width::U16, hello.block_len());
        for extension in &hello.extensions {
            dst.put_u16(extension.extension_type().get());
            write_length(dst, Width::U16, extension.data_len());
            write_extension(dst, extension);
        }
    }

    /// Writes the data of `extension`.
    fn write_extension<C: AsRef<[u8]>>(dst: &mut impl BufMut, extension: &ClientHelloExtension<C>) {
        use ClientHelloExtension as X;
        match extension {
            X::ServerName(name) => name.encode(dst),
            X::SupportedGroups(list) => write_code_point_list(dst, list),
            X::SignatureAlgorithms(list) | X::SignatureAlgorithmsCert(list) => {
                write_code_point_list(dst, list)
            }
            X::ApplicationLayerProtocolNegotiation(list) => list.encode(dst),
            X::SignedCertificateTimestamp | X::EarlyData => {}
            X::ClientCertificateType(list) | X::ServerCertificateType(list) => {
                write_byte_code_point_list(dst, list)
            }
            X::PreSharedKey(offered) => offered.encode(dst),
            X::SupportedVersions(list) => write_code_point_list(dst, list),
            X::Cookie(cookie) => cookie.encode(dst),
            X::PskKeyExchangeModes(list) => write_byte_code_point_list(dst, list),
            X::KeyShare(shares) => shares.encode(dst),
            X::QuicTransportParameters(data) | X::EncryptedClientHello(data) => {
                dst.put_slice(data.data().as_ref())
            }
            X::Unknown(extension) => dst.put_slice(extension.data().as_ref()),
        }
    }
}

#[cfg(test)]
mod tests;

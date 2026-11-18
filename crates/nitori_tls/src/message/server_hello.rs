//! ServerHello and HelloRetryRequest (RFC 9846 §4.2.3, §4.2.4).
//!
//! Both are server_hello messages; a HelloRetryRequest is the one whose
//! random is [`HELLO_RETRY_REQUEST_RANDOM`]. [`codec::read_server_hello_message`]
//! tells them apart and reads the extensions each may carry.

use crate::extension::{OpaqueData, extension_subset};
use crate::{
    cipher_suite::CipherSuite,
    extension::{
        BlockEntry, ExtensionType, InvalidExtensionBlockError, UnknownExtension, check_block,
        cookie::Cookie, key_share::KeyShareEntry, pre_shared_key::PskIndex,
    },
    message::{HelloRandom, LegacySessionId},
    version::ProtocolVersion,
    width::Width,
};
use nitori_crypto::NamedGroup;
use snafu::{ResultExt, Snafu};

/// The random of every HelloRetryRequest: SHA-256 of "HelloRetryRequest"
/// (RFC 9846 §4.2.3).
pub const HELLO_RETRY_REQUEST_RANDOM: HelloRandom = HelloRandom([
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91,
    0xc2, 0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
]);

/// The lengths an extension block of either message may have.
const BLOCK: core::ops::RangeInclusive<usize> = 6..=Width::U16.max();

/// A ServerHello.
///
/// `legacy_compression_method` is always 0, so the value does not hold it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerHello<C> {
    legacy_version: ProtocolVersion,
    random: HelloRandom,
    legacy_session_id_echo: LegacySessionId,
    cipher_suite: CipherSuite,
    extensions: Vec<ServerHelloExtension<C>>,
}

/// An extension of a ServerHello.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ServerHelloExtension<C> {
    /// `key_share`: the server's share.
    KeyShare(KeyShareEntry<C>),
    /// `pre_shared_key`: the index of the selected identity.
    PreSharedKey(PskIndex),
    /// `supported_versions`: the selected version.
    SupportedVersions(ProtocolVersion),
    /// An extension the readers do not recognize.
    Unknown(UnknownExtension<C>),
}

impl<C: AsRef<[u8]>> BlockEntry for ServerHelloExtension<C> {
    fn extension_type(&self) -> ExtensionType {
        match self {
            Self::KeyShare(_) => ExtensionType::KEY_SHARE,
            Self::PreSharedKey(_) => ExtensionType::PRE_SHARED_KEY,
            Self::SupportedVersions(_) => ExtensionType::SUPPORTED_VERSIONS,
            Self::Unknown(extension) => extension.extension_type(),
        }
    }

    fn data_len(&self) -> usize {
        match self {
            Self::KeyShare(entry) => entry.encoded_len(),
            Self::PreSharedKey(_) | Self::SupportedVersions(_) => 2,
            Self::Unknown(extension) => extension.data().as_ref().len(),
        }
    }
}

impl<C: AsRef<[u8]>> ServerHello<C> {
    /// The ServerHello with these fields.
    ///
    /// # Errors
    ///
    /// [`InvalidServerHelloError::RetryRandom`] for the random of a
    /// HelloRetryRequest, and [`InvalidServerHelloError::Extensions`] for an
    /// extension type that appears twice or
    /// a block outside 6 to 65535 bytes.
    pub fn new(
        legacy_version: ProtocolVersion,
        random: HelloRandom,
        legacy_session_id_echo: LegacySessionId,
        cipher_suite: CipherSuite,
        extensions: Vec<ServerHelloExtension<C>>,
    ) -> Result<Self, InvalidServerHelloError> {
        if random == HELLO_RETRY_REQUEST_RANDOM {
            return invalid_server_hello::RetryRandomSnafu.fail();
        }
        check_block(&extensions, BLOCK).context(invalid_server_hello::ExtensionsSnafu)?;
        Ok(Self {
            legacy_version,
            random,
            legacy_session_id_echo,
            cipher_suite,
            extensions,
        })
    }

    fn block_len(&self) -> usize {
        block_len(&self.extensions)
    }

    fn body_len(&self) -> usize {
        body_len(&self.legacy_session_id_echo, self.block_len())
    }

    /// The number of bytes [`codec::write_server_hello`] writes, the
    /// message header included.
    pub fn encoded_len(&self) -> usize {
        4 + self.body_len()
    }
}

impl<C> ServerHello<C> {
    /// The legacy_version field.
    pub fn legacy_version(&self) -> ProtocolVersion {
        self.legacy_version
    }

    /// The random field.
    pub fn random(&self) -> &HelloRandom {
        &self.random
    }

    /// The legacy_session_id_echo field.
    pub fn legacy_session_id_echo(&self) -> &LegacySessionId {
        &self.legacy_session_id_echo
    }

    /// The selected cipher suite.
    pub fn cipher_suite(&self) -> CipherSuite {
        self.cipher_suite
    }

    /// The extensions, in the order of the wire.
    pub fn extensions(&self) -> &[ServerHelloExtension<C>] {
        &self.extensions
    }

    /// The server's share, if present.
    pub fn key_share(&self) -> Option<&KeyShareEntry<C>> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                ServerHelloExtension::KeyShare(entry) => Some(entry),
                _ => None,
            })
    }

    /// The index of the selected PSK, if present.
    pub fn pre_shared_key(&self) -> Option<PskIndex> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                ServerHelloExtension::PreSharedKey(index) => Some(*index),
                _ => None,
            })
    }

    /// The selected version, if present.
    pub fn supported_versions(&self) -> Option<ProtocolVersion> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                ServerHelloExtension::SupportedVersions(version) => Some(*version),
                _ => None,
            })
    }
}

/// Failures of [`ServerHello::new`].
#[derive(Debug, Snafu)]
#[snafu(module(invalid_server_hello), visibility(pub(crate)))]
pub enum InvalidServerHelloError {
    /// The random is that of a HelloRetryRequest.
    #[snafu(display("ServerHello random is that of a HelloRetryRequest"))]
    RetryRandom,
    /// The extension block is invalid.
    #[snafu(display("ServerHello extensions invalid"))]
    Extensions {
        /// Why.
        source: InvalidExtensionBlockError,
    },
}

/// A HelloRetryRequest. Its random is [`HELLO_RETRY_REQUEST_RANDOM`], and
/// `legacy_compression_method` is always 0.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HelloRetryRequest<C> {
    legacy_version: ProtocolVersion,
    legacy_session_id_echo: LegacySessionId,
    cipher_suite: CipherSuite,
    extensions: Vec<HelloRetryRequestExtension<C>>,
}

/// An extension of a HelloRetryRequest.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HelloRetryRequestExtension<C> {
    /// `key_share`: the group the server selected.
    KeyShare(NamedGroup),
    /// `cookie`.
    Cookie(Cookie<C>),
    /// `supported_versions`: the selected version.
    SupportedVersions(ProtocolVersion),
    /// `encrypted_client_hello`, as opaque bytes.
    EncryptedClientHello(OpaqueData<C>),
    /// An extension the readers do not recognize.
    Unknown(UnknownExtension<C>),
}

impl<C: AsRef<[u8]>> BlockEntry for HelloRetryRequestExtension<C> {
    fn extension_type(&self) -> ExtensionType {
        match self {
            Self::KeyShare(_) => ExtensionType::KEY_SHARE,
            Self::Cookie(_) => ExtensionType::COOKIE,
            Self::SupportedVersions(_) => ExtensionType::SUPPORTED_VERSIONS,
            Self::EncryptedClientHello(_) => ExtensionType::ENCRYPTED_CLIENT_HELLO,
            Self::Unknown(extension) => extension.extension_type(),
        }
    }

    fn data_len(&self) -> usize {
        match self {
            Self::KeyShare(_) | Self::SupportedVersions(_) => 2,
            Self::Cookie(cookie) => cookie.encoded_len(),
            Self::EncryptedClientHello(data) => data.data().as_ref().len(),
            Self::Unknown(extension) => extension.data().as_ref().len(),
        }
    }
}

impl<C: AsRef<[u8]>> HelloRetryRequest<C> {
    /// The HelloRetryRequest with these fields.
    ///
    /// # Errors
    ///
    /// [`InvalidExtensionBlockError`] for an extension type that appears twice or a block outside 6 to 65535 bytes.
    pub fn new(
        legacy_version: ProtocolVersion,
        legacy_session_id_echo: LegacySessionId,
        cipher_suite: CipherSuite,
        extensions: Vec<HelloRetryRequestExtension<C>>,
    ) -> Result<Self, InvalidExtensionBlockError> {
        check_block(&extensions, BLOCK)?;
        Ok(Self {
            legacy_version,
            legacy_session_id_echo,
            cipher_suite,
            extensions,
        })
    }

    fn block_len(&self) -> usize {
        block_len(&self.extensions)
    }

    fn body_len(&self) -> usize {
        body_len(&self.legacy_session_id_echo, self.block_len())
    }

    /// The number of bytes [`codec::write_hello_retry_request`] writes, the
    /// message header included.
    pub fn encoded_len(&self) -> usize {
        4 + self.body_len()
    }
}

impl<C> HelloRetryRequest<C> {
    /// The legacy_version field.
    pub fn legacy_version(&self) -> ProtocolVersion {
        self.legacy_version
    }

    /// The legacy_session_id_echo field.
    pub fn legacy_session_id_echo(&self) -> &LegacySessionId {
        &self.legacy_session_id_echo
    }

    /// The selected cipher suite.
    pub fn cipher_suite(&self) -> CipherSuite {
        self.cipher_suite
    }

    /// The extensions, in the order of the wire.
    pub fn extensions(&self) -> &[HelloRetryRequestExtension<C>] {
        &self.extensions
    }

    /// The selected group, if present.
    pub fn key_share(&self) -> Option<NamedGroup> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                HelloRetryRequestExtension::KeyShare(group) => Some(*group),
                _ => None,
            })
    }

    /// The cookie, if present.
    pub fn cookie(&self) -> Option<&Cookie<C>> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                HelloRetryRequestExtension::Cookie(cookie) => Some(cookie),
                _ => None,
            })
    }
}

/// A server_hello message: a ServerHello or a HelloRetryRequest.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ServerHelloMessage<C> {
    /// A ServerHello.
    ServerHello(ServerHello<C>),
    /// A HelloRetryRequest.
    HelloRetryRequest(HelloRetryRequest<C>),
}

fn block_len<X: BlockEntry>(extensions: &[X]) -> usize {
    extensions
        .iter()
        .map(|extension| 4 + extension.data_len())
        .sum()
}

fn body_len(session_id: &LegacySessionId, block_len: usize) -> usize {
    2 + 32 + session_id.encoded_len() + 2 + 1 + 2 + block_len
}

extension_subset! {
    /// A recognized extension that a ServerHello may not carry: every one but `key_share`,
    /// `pre_shared_key` and `supported_versions`.
    ServerHelloDisallowedExtension {
        ServerName,
        SupportedGroups,
        SignatureAlgorithms,
        ApplicationLayerProtocolNegotiation,
        SignedCertificateTimestamp,
        ClientCertificateType,
        ServerCertificateType,
        EarlyData,
        Cookie,
        PskKeyExchangeModes,
        SignatureAlgorithmsCert,
        QuicTransportParameters,
        EncryptedClientHello,
    }
}

extension_subset! {
    /// A recognized extension that a HelloRetryRequest may not carry: every one but `key_share`,
    /// `cookie`, `supported_versions` and `encrypted_client_hello`.
    HelloRetryRequestDisallowedExtension {
        ServerName,
        SupportedGroups,
        SignatureAlgorithms,
        ApplicationLayerProtocolNegotiation,
        SignedCertificateTimestamp,
        ClientCertificateType,
        ServerCertificateType,
        PreSharedKey,
        EarlyData,
        PskKeyExchangeModes,
        SignatureAlgorithmsCert,
        QuicTransportParameters,
    }
}

extension_subset! {
    /// An extension a ServerHello may carry whose data has a structure, so that bytes can follow
    /// it: every extension it may carry.
    ServerHelloStructuredExtension {
        PreSharedKey,
        SupportedVersions,
        KeyShare,
    }
}

extension_subset! {
    /// An extension a HelloRetryRequest may carry whose data has a structure, so that bytes can
    /// follow it: every extension it may carry but `encrypted_client_hello`, whose data is kept
    /// whole.
    HelloRetryRequestStructuredExtension {
        SupportedVersions,
        Cookie,
        KeyShare,
    }
}

pub mod codec {
    //! Reading and writing server_hello messages.

    use super::{
        BLOCK, HELLO_RETRY_REQUEST_RANDOM, HelloRetryRequest, HelloRetryRequestDisallowedExtension,
        HelloRetryRequestExtension, ServerHello, ServerHelloDisallowedExtension,
        ServerHelloExtension, ServerHelloMessage,
    };
    use crate::{
        alert::AlertDescription,
        cipher_suite::CipherSuite,
        extension::{
            BlockEntry as _, BlockReader, ExtensionNotAllowedError, ReadExtensionBlockError,
            RestrictedExtension, UnknownExtension,
            cookie::codec::read_cookie,
            excess,
            key_share::codec::{ReadKeyShareEntryError, read_key_share_entry},
            not_allowed,
            pre_shared_key::PskIndex,
            read_extension_block,
        },
        message::{HandshakeType, HelloRandom, ReadHeaderError, read_body, read_session_id},
        source::{read_array, read_u8, read_u16, take_rest},
        vector::{
            ExcessBytesError, LengthOutOfRangeError, ReadBoundedVectorError, write_header,
            write_length, write_vector,
        },
        version::ProtocolVersion,
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_crypto::NamedGroup;
    use nitori_io::{SplitTo, error::IncompleteError};
    use snafu::{ResultExt, Snafu};

    /// Reads a server_hello message, header included: a ServerHello, or a
    /// HelloRetryRequest when the random says so.
    ///
    /// # Errors
    ///
    /// [`ReadServerHelloMessageError`].
    pub fn read_server_hello_message<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ServerHelloMessage<S>, ReadServerHelloMessageError> {
        let mut body = read_body(src, HandshakeType::SERVER_HELLO)?;
        let hello = read_server_hello_message_body(&mut body)?;
        ExcessBytesError::check(body.remaining())
            .context(read_server_hello_message_error::ExcessSnafu)?;
        Ok(hello)
    }

    fn read_server_hello_message_body<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ServerHelloMessage<S>, ReadServerHelloMessageError> {
        use read_server_hello_message_error::*;
        let legacy_version = ProtocolVersion::new(read_u16(src).context(LegacyVersionSnafu)?);
        let random = HelloRandom(read_array(src).context(RandomSnafu)?);
        let legacy_session_id_echo = read_session_id(src).context(SessionIdSnafu)?;
        let cipher_suite = CipherSuite::new(read_u16(src).context(CipherSuiteSnafu)?);
        if read_u8(src).context(CompressionMethodSnafu)? != 0 {
            return CompressionMethodNotNullSnafu.fail();
        }
        let block = read_extension_block(src).context(ExtensionsSnafu)?;
        LengthOutOfRangeError::check(block.remaining(), BLOCK).context(ExtensionsLengthSnafu)?;
        let mut block = BlockReader::new(block);
        let retry = random == HELLO_RETRY_REQUEST_RANDOM;
        let mut server = Vec::new();
        let mut retry_extensions = Vec::new();
        while let Some((extension_type, mut data)) = block.next().context(ExtensionsSnafu)? {
            let Some(restricted) = extension_type.restricted() else {
                let extension = UnknownExtension::read(extension_type, data);
                match retry {
                    false => server.push(ServerHelloExtension::Unknown(extension)),
                    true => retry_extensions.push(HelloRetryRequestExtension::Unknown(extension)),
                }
                continue;
            };
            match retry {
                false => {
                    server.push(read_server_extension(restricted, &mut data)?);
                    let left = data.remaining();
                    let of = super::ServerHelloStructuredExtension::of;
                    if let Some((extension, source)) = excess(restricted, left, of) {
                        return Err(ReadServerHelloMessageError::ExtensionExcess {
                            extension,
                            source,
                        });
                    }
                }
                true => {
                    retry_extensions.push(read_retry_extension(restricted, &mut data)?);
                    let left = data.remaining();
                    let of = super::HelloRetryRequestStructuredExtension::of;
                    if let Some((extension, source)) = excess(restricted, left, of) {
                        return Err(ReadServerHelloMessageError::RetryExtensionExcess {
                            extension,
                            source,
                        });
                    }
                }
            }
        }
        Ok(match retry {
            false => ServerHelloMessage::ServerHello(ServerHello {
                legacy_version,
                random,
                legacy_session_id_echo,
                cipher_suite,
                extensions: server,
            }),
            true => ServerHelloMessage::HelloRetryRequest(HelloRetryRequest {
                legacy_version,
                legacy_session_id_echo,
                cipher_suite,
                extensions: retry_extensions,
            }),
        })
    }

    /// Reads the data of a recognized extension of a ServerHello.
    fn read_server_extension<B>(
        extension: RestrictedExtension,
        data: &mut B,
    ) -> Result<ServerHelloExtension<B>, ReadServerHelloMessageError>
    where
        B: Buf + SplitTo,
    {
        use RestrictedExtension as R;
        use read_server_hello_message_error::*;
        Ok(match extension {
            R::KeyShare => {
                ServerHelloExtension::KeyShare(read_key_share_entry(data).context(KeyShareSnafu)?)
            }
            R::PreSharedKey => ServerHelloExtension::PreSharedKey(PskIndex::new(
                read_u16(data).context(SelectedIdentitySnafu)?,
            )),
            R::SupportedVersions => ServerHelloExtension::SupportedVersions(ProtocolVersion::new(
                read_u16(data).context(SelectedVersionSnafu)?,
            )),
            other => return Err(not_allowed(other, ServerHelloDisallowedExtension::of).into()),
        })
    }

    /// Reads the data of a recognized extension of a HelloRetryRequest.
    fn read_retry_extension<B>(
        extension: RestrictedExtension,
        data: &mut B,
    ) -> Result<HelloRetryRequestExtension<B>, ReadServerHelloMessageError>
    where
        B: Buf + SplitTo,
    {
        use RestrictedExtension as R;
        use read_server_hello_message_error::*;
        Ok(match extension {
            R::KeyShare => HelloRetryRequestExtension::KeyShare(NamedGroup::new(
                read_u16(data).context(SelectedGroupSnafu)?,
            )),
            R::Cookie => {
                HelloRetryRequestExtension::Cookie(read_cookie(data).context(CookieSnafu)?)
            }
            R::SupportedVersions => HelloRetryRequestExtension::SupportedVersions(
                ProtocolVersion::new(read_u16(data).context(SelectedVersionSnafu)?),
            ),
            R::EncryptedClientHello => HelloRetryRequestExtension::EncryptedClientHello(
                crate::extension::OpaqueData::read(take_rest(data)),
            ),
            other => {
                return Err(not_allowed(other, HelloRetryRequestDisallowedExtension::of).into());
            }
        })
    }

    /// Failures that [`read_server_hello_message`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_server_hello_message_error), visibility(pub(crate)))]
    pub enum ReadServerHelloMessageError {
        /// The header could not be read, or the message is of another type.
        #[snafu(transparent)]
        Header {
            /// The failure.
            source: ReadHeaderError,
        },
        /// The body is longer than its fields.
        #[snafu(display("ServerHello body has excess bytes"))]
        Excess {
            /// The excess.
            source: ExcessBytesError,
        },
        /// legacy_version could not be read.
        #[snafu(display("ServerHello legacy_version read failed"))]
        LegacyVersion {
            /// The shortfall.
            source: IncompleteError,
        },
        /// random could not be read.
        #[snafu(display("ServerHello random read failed"))]
        Random {
            /// The shortfall.
            source: IncompleteError,
        },
        /// legacy_session_id_echo could not be read or is longer than 32
        /// bytes.
        #[snafu(display("ServerHello legacy_session_id_echo read failed"))]
        SessionId {
            /// The failure of reading the field.
            source: ReadBoundedVectorError,
        },
        /// cipher_suite could not be read.
        #[snafu(display("ServerHello cipher_suite read failed"))]
        CipherSuite {
            /// The shortfall.
            source: IncompleteError,
        },
        /// legacy_compression_method could not be read.
        #[snafu(display("ServerHello legacy_compression_method read failed"))]
        CompressionMethod {
            /// The shortfall.
            source: IncompleteError,
        },
        /// legacy_compression_method is not 0 (RFC 9846 §4.2.3).
        #[snafu(display("ServerHello legacy_compression_method is not 0"))]
        CompressionMethodNotNull,
        /// The extension block could not be read, or a type appears twice.
        #[snafu(display("ServerHello extensions read failed"))]
        Extensions {
            /// The failure.
            source: ReadExtensionBlockError,
        },
        /// The extension block is shorter than 6 bytes.
        #[snafu(display("ServerHello extension block length out of range"))]
        ExtensionsLength {
            /// The broken range.
            source: LengthOutOfRangeError,
        },
        /// A recognized extension that a ServerHello may not carry.
        #[snafu(transparent)]
        ExtensionNotAllowed {
            /// The extension.
            source: ExtensionNotAllowedError<ServerHelloDisallowedExtension>,
        },
        /// A recognized extension that a HelloRetryRequest may not carry.
        #[snafu(transparent)]
        RetryExtensionNotAllowed {
            /// The extension.
            source: ExtensionNotAllowedError<HelloRetryRequestDisallowedExtension>,
        },
        /// The data of an extension of a ServerHello is longer than its
        /// fields.
        #[snafu(display("{extension:?} data has excess bytes"))]
        ExtensionExcess {
            /// The extension.
            extension: crate::message::server_hello::ServerHelloStructuredExtension,
            /// The excess.
            source: ExcessBytesError,
        },
        /// The data of an extension of a HelloRetryRequest is longer than its
        /// fields.
        #[snafu(display("{extension:?} data has excess bytes"))]
        RetryExtensionExcess {
            /// The extension.
            extension: crate::message::server_hello::HelloRetryRequestStructuredExtension,
            /// The excess.
            source: ExcessBytesError,
        },
        /// `key_share` of a ServerHello could not be read.
        #[snafu(display("key_share read failed"))]
        KeyShare {
            /// The failure.
            source: ReadKeyShareEntryError,
        },
        /// `key_share` of a HelloRetryRequest could not be read.
        #[snafu(display("key_share selected_group read failed"))]
        SelectedGroup {
            /// The shortfall.
            source: IncompleteError,
        },
        /// `pre_shared_key` could not be read.
        #[snafu(display("pre_shared_key selected_identity read failed"))]
        SelectedIdentity {
            /// The shortfall.
            source: IncompleteError,
        },
        /// `supported_versions` could not be read.
        #[snafu(display("supported_versions selected_version read failed"))]
        SelectedVersion {
            /// The shortfall.
            source: IncompleteError,
        },
        /// `cookie` could not be read.
        #[snafu(display("cookie read failed"))]
        Cookie {
            /// The failure.
            source: ReadBoundedVectorError,
        },
    }

    impl ReadServerHelloMessageError {
        /// The alert the failure calls for.
        ///
        /// illegal_parameter for a compression method other than 0, for a
        /// type that appears twice and for a recognized extension this
        /// message may not carry (RFC 9846 §4.3); decode_error for every
        /// failure to decode.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::Header { source } => source.alert(),
                Self::CompressionMethodNotNull => AlertDescription::ILLEGAL_PARAMETER,
                Self::Extensions { source } => source.alert(),
                Self::ExtensionNotAllowed { source } => source.alert(),
                Self::RetryExtensionNotAllowed { source } => source.alert(),
                Self::Excess { .. }
                | Self::LegacyVersion { .. }
                | Self::Random { .. }
                | Self::SessionId { .. }
                | Self::CipherSuite { .. }
                | Self::CompressionMethod { .. }
                | Self::ExtensionsLength { .. }
                | Self::ExtensionExcess { .. }
                | Self::RetryExtensionExcess { .. }
                | Self::KeyShare { .. }
                | Self::SelectedGroup { .. }
                | Self::SelectedIdentity { .. }
                | Self::SelectedVersion { .. }
                | Self::Cookie { .. } => AlertDescription::DECODE_ERROR,
            }
        }
    }

    /// Writes `hello`, header included.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_server_hello<C: AsRef<[u8]>>(dst: &mut impl BufMut, hello: &ServerHello<C>) {
        write_fields(
            dst,
            hello.body_len(),
            hello.legacy_version,
            &hello.random,
            hello.legacy_session_id_echo.as_bytes(),
            hello.cipher_suite,
            hello.block_len(),
        );
        for extension in &hello.extensions {
            dst.put_u16(extension.extension_type().get());
            write_length(dst, Width::U16, extension.data_len());
            match extension {
                ServerHelloExtension::KeyShare(entry) => entry.encode(dst),
                ServerHelloExtension::PreSharedKey(index) => dst.put_u16(index.get()),
                ServerHelloExtension::SupportedVersions(version) => dst.put_u16(version.get()),
                ServerHelloExtension::Unknown(extension) => {
                    dst.put_slice(extension.data().as_ref())
                }
            }
        }
    }

    /// Writes `retry`, header included.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_hello_retry_request<C: AsRef<[u8]>>(
        dst: &mut impl BufMut,
        retry: &HelloRetryRequest<C>,
    ) {
        write_fields(
            dst,
            retry.body_len(),
            retry.legacy_version,
            &HELLO_RETRY_REQUEST_RANDOM,
            retry.legacy_session_id_echo.as_bytes(),
            retry.cipher_suite,
            retry.block_len(),
        );
        for extension in &retry.extensions {
            dst.put_u16(extension.extension_type().get());
            write_length(dst, Width::U16, extension.data_len());
            match extension {
                HelloRetryRequestExtension::KeyShare(group) => dst.put_u16(group.get()),
                HelloRetryRequestExtension::Cookie(cookie) => cookie.encode(dst),
                HelloRetryRequestExtension::SupportedVersions(version) => {
                    dst.put_u16(version.get())
                }
                HelloRetryRequestExtension::EncryptedClientHello(data) => {
                    dst.put_slice(data.data().as_ref())
                }
                HelloRetryRequestExtension::Unknown(extension) => {
                    dst.put_slice(extension.data().as_ref())
                }
            }
        }
    }

    /// Writes the header and the fields before the extensions, and the
    /// length of the extension block.
    fn write_fields(
        dst: &mut impl BufMut,
        body_len: usize,
        legacy_version: ProtocolVersion,
        random: &HelloRandom,
        session_id: &[u8],
        cipher_suite: CipherSuite,
        block_len: usize,
    ) {
        write_header(dst, HandshakeType::SERVER_HELLO, body_len);
        dst.put_u16(legacy_version.get());
        dst.put_slice(&random.0[..]);
        write_vector(dst, Width::U8, session_id);
        dst.put_u16(cipher_suite.get());
        dst.put_u8(0);
        write_length(dst, Width::U16, block_len)
    }
}

#[cfg(test)]
mod tests;

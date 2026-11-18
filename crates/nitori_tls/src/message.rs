//! TLS 1.3 handshake messages (RFC 9846 §4).
//!
//! A handshake message is a one-byte [`HandshakeType`], a three-byte length
//! and a body of that length. In QUIC the messages of each encryption level
//! form one byte stream, carried in CRYPTO frames (RFC 9001 §4).
//!
//! [`codec::read_handshake_message`] collects one whole message from that
//! stream into a [`HandshakeMessage`], refusing a length above the
//! [`MessageLimits`] for its type before reading the body. Each message type
//! then has its own module with a reader and a writer, synchronous functions
//! that run on the bytes of one message: a reader takes a source that is a
//! [`Buf`] and splits with [`SplitTo`], such as `&[u8]`
//! or `Bytes`, and its variable-length fields are pieces of that source,
//! subslices or `Bytes`, without copying; a writer takes a
//! [`BufMut`](bytes::BufMut). A reader checks the encoding and the rules of RFC 9846
//! that one message states by itself; rules that relate a message to others,
//! or to what an endpoint offered, are for the handshake to check.
//!
//! - ClientHello in [`client_hello`];
//! - ServerHello and HelloRetryRequest, which share a type, in
//!   [`server_hello`];
//! - EncryptedExtensions in [`encrypted_extensions`];
//! - CertificateRequest in [`certificate_request`];
//! - Certificate, with X.509 certificates or a raw public key, in
//!   [`certificate`];
//! - CertificateVerify in [`certificate_verify`];
//! - Finished in [`finished`];
//! - NewSessionTicket in [`new_session_ticket`];
//! - the message_hash message, which stands for the first ClientHello in the
//!   transcript after a HelloRetryRequest, in [`message_hash`].
//!
//! Every read failure answers the alert it calls for through `alert`.

use crate::{
    alert::AlertDescription,
    code_point::code_point,
    source::{read_bytes, read_u8, read_u24},
    vector::{LengthOutOfRangeError, ReadBoundedVectorError, read_bounded_vector},
    width::Width,
};
use bytes::{Buf, Bytes};
use nitori_crypto::HashAlgorithm;
use nitori_io::{SplitTo, error::IncompleteError};
use snafu::{ResultExt, Snafu};

pub mod certificate;
pub mod certificate_request;
pub mod certificate_verify;
pub mod client_hello;
pub mod encrypted_extensions;
pub mod finished;
pub mod message_hash;
pub mod new_session_ticket;
pub mod server_hello;

code_point! {
    /// A HandshakeType code point (RFC 9846 §4).
    ///
    /// Any byte is a code point.
    HandshakeType(u8);
    /// client_hello.
    CLIENT_HELLO = 1 => "client_hello",
    /// server_hello, also the type of HelloRetryRequest.
    SERVER_HELLO = 2 => "server_hello",
    /// new_session_ticket.
    NEW_SESSION_TICKET = 4 => "new_session_ticket",
    /// end_of_early_data, which QUIC does not use (RFC 9001 §8.3).
    END_OF_EARLY_DATA = 5 => "end_of_early_data",
    /// encrypted_extensions.
    ENCRYPTED_EXTENSIONS = 8 => "encrypted_extensions",
    /// certificate.
    CERTIFICATE = 11 => "certificate",
    /// certificate_request.
    CERTIFICATE_REQUEST = 13 => "certificate_request",
    /// certificate_verify.
    CERTIFICATE_VERIFY = 15 => "certificate_verify",
    /// finished.
    FINISHED = 20 => "finished",
    /// key_update, which QUIC does not use (RFC 9001 §6).
    KEY_UPDATE = 24 => "key_update",
    /// message_hash, which appears only in the transcript.
    MESSAGE_HASH = 254 => "message_hash",
}

/// Failures of reading the header of a handshake message of a given type
/// and taking its body.
#[derive(Debug, Snafu)]
#[snafu(module(read_header_error), visibility(pub(crate)))]
pub enum ReadHeaderError {
    /// The message type could not be read.
    #[snafu(display("handshake message type read failed"))]
    Type {
        /// The shortfall.
        source: IncompleteError,
    },
    /// The message is not of the type the reader reads.
    #[snafu(transparent)]
    UnexpectedType {
        /// The types.
        source: UnexpectedTypeError,
    },
    /// The length could not be read.
    #[snafu(display("handshake message length read failed"))]
    Length {
        /// The shortfall.
        source: IncompleteError,
    },
    /// The body is shorter than the length states.
    #[snafu(display("handshake message body read failed"))]
    Body {
        /// The shortfall.
        source: IncompleteError,
    },
}

impl ReadHeaderError {
    /// The alert the failure calls for: unexpected_message for a message of
    /// another type, decode_error otherwise.
    pub fn alert(&self) -> AlertDescription {
        match self {
            Self::UnexpectedType { .. } => AlertDescription::UNEXPECTED_MESSAGE,
            Self::Type { .. } | Self::Length { .. } | Self::Body { .. } => {
                AlertDescription::DECODE_ERROR
            }
        }
    }
}

/// A message is not of the type a reader reads; the error of each reader
/// says which type that is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("message of another type, {found:?}"))]
pub struct UnexpectedTypeError {
    found: HandshakeType,
}

impl UnexpectedTypeError {
    /// The type of the message.
    pub const fn found(self) -> HandshakeType {
        self.found
    }
}

/// Reads the header of a message of type `expected` and returns its body.
pub(crate) fn read_body<S: Buf + SplitTo>(
    src: &mut S,
    expected: HandshakeType,
) -> Result<S, ReadHeaderError> {
    use read_header_error::*;
    let found = HandshakeType(read_u8(src).context(TypeSnafu)?);
    if found != expected {
        return Err(UnexpectedTypeError { found }.into());
    }
    let len = read_u24(src).context(LengthSnafu)?;
    read_bytes(src, len as usize).context(BodySnafu)
}

/// The random field of a ClientHello or ServerHello: 32 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HelloRandom(pub [u8; 32]);

/// The legacy_session_id of a ClientHello, or its echo in a ServerHello:
/// 0 to 32 bytes.
///
/// QUIC forbids the middlebox compatibility mode, so a QUIC client sends an
/// empty one (RFC 9001 §8.4); the readers keep what they read.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LegacySessionId {
    len: u8,
    bytes: [u8; LegacySessionId::MAX_LEN],
}

impl LegacySessionId {
    /// The longest session ID, in bytes.
    pub const MAX_LEN: usize = 32;

    /// The empty session ID.
    pub const EMPTY: Self = Self {
        len: 0,
        bytes: [0; Self::MAX_LEN],
    };

    /// Copies a session ID from `bytes`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] if `bytes` is longer than 32.
    pub fn new(bytes: &[u8]) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(bytes.len(), 0..=Self::MAX_LEN)?;
        let mut id = Self::EMPTY;
        id.bytes[..bytes.len()].copy_from_slice(bytes);
        id.len = bytes.len() as u8;
        Ok(id)
    }

    /// The bytes of the session ID.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// The number of bytes the session ID takes with its length field.
    pub(crate) fn encoded_len(&self) -> usize {
        1 + usize::from(self.len)
    }
}

impl core::fmt::Debug for LegacySessionId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("LegacySessionId(")?;
        for byte in self.as_bytes() {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

/// Reads a session ID with its one-byte length.
pub(crate) fn read_session_id<S: Buf + SplitTo>(
    src: &mut S,
) -> Result<LegacySessionId, ReadBoundedVectorError> {
    let mut bytes = read_bounded_vector(src, Width::U8, 0..=LegacySessionId::MAX_LEN)?;
    let len = bytes.remaining();
    let mut id = LegacySessionId::EMPTY;
    bytes.copy_to_slice(&mut id.bytes[..len]);
    // invariant: the range was checked.
    id.len = len as u8;
    Ok(id)
}

/// The largest body a handshake message of one type may have, in bytes,
/// below the 2<sup>24</sup> − 1 its length field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageLimit(u32);

impl MessageLimit {
    /// The largest limit, 2<sup>24</sup> − 1 bytes.
    pub const MAX: Self = Self(0xff_ffff);

    /// A limit of `len` bytes.
    ///
    /// # Errors
    ///
    /// [`MessageLimitTooLargeError`] if `len` is above 2<sup>24</sup> − 1.
    pub const fn new(len: u32) -> Result<Self, MessageLimitTooLargeError> {
        if len > Self::MAX.0 {
            return Err(MessageLimitTooLargeError { len });
        }
        Ok(Self(len))
    }

    /// A limit of `kib` × 1024 bytes, for the defaults.
    const fn kib(kib: u32) -> Self {
        Self(kib * 1024)
    }

    /// The limit in bytes.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// A message limit above what the length field of a handshake message holds.
///
/// It is constructed only when the check fails, so the length is always
/// above 2<sup>24</sup> − 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("message limit of {len} bytes exceeds 16777215"))]
pub struct MessageLimitTooLargeError {
    len: u32,
}

impl MessageLimitTooLargeError {
    /// The rejected length.
    pub const fn length(self) -> u32 {
        self.len
    }
}

/// The limits on the body of each type of handshake message an endpoint
/// receives.
///
/// [`codec::read_handshake_message`] collects a message into contiguous
/// memory before it is read, and CRYPTO frames have no flow control
/// (RFC 9000 §7.5), so each type has a limit. The body of a Finished message
/// is as long as the hash of the cipher suite (RFC 9846 §4.5.3) and needs no
/// limit of its own.
///
/// The defaults follow sizes seen in practice and may change as traffic or
/// tests show them unsuitable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MessageLimits {
    /// ClientHello; 16 KiB by default.
    pub client_hello: MessageLimit,
    /// ServerHello and HelloRetryRequest; 16 KiB by default.
    pub server_hello: MessageLimit,
    /// EncryptedExtensions; 16 KiB by default.
    pub encrypted_extensions: MessageLimit,
    /// CertificateRequest; 16 KiB by default.
    pub certificate_request: MessageLimit,
    /// Certificate; 64 KiB by default.
    pub certificate: MessageLimit,
    /// CertificateVerify; 8 KiB by default.
    pub certificate_verify: MessageLimit,
    /// NewSessionTicket; 16 KiB by default.
    pub new_session_ticket: MessageLimit,
}

impl Default for MessageLimits {
    fn default() -> Self {
        Self {
            client_hello: MessageLimit::kib(16),
            server_hello: MessageLimit::kib(16),
            encrypted_extensions: MessageLimit::kib(16),
            certificate_request: MessageLimit::kib(16),
            certificate: MessageLimit::kib(64),
            certificate_verify: MessageLimit::kib(8),
            new_session_ticket: MessageLimit::kib(16),
        }
    }
}

impl MessageLimits {
    /// The limit on the body of a message of type `message`, with the hash
    /// of the cipher suite once it is known, or `None` for Finished before
    /// the suite is known.
    pub const fn limit(
        &self,
        message: ReceivedMessageType,
        hash: Option<HashAlgorithm>,
    ) -> Option<u32> {
        Some(match message {
            ReceivedMessageType::ClientHello => self.client_hello.0,
            ReceivedMessageType::ServerHello => self.server_hello.0,
            ReceivedMessageType::EncryptedExtensions => self.encrypted_extensions.0,
            ReceivedMessageType::CertificateRequest => self.certificate_request.0,
            ReceivedMessageType::Certificate => self.certificate.0,
            ReceivedMessageType::CertificateVerify => self.certificate_verify.0,
            ReceivedMessageType::NewSessionTicket => self.new_session_ticket.0,
            ReceivedMessageType::Finished => match hash {
                Some(hash) => hash.output_len() as u32,
                None => return None,
            },
        })
    }
}

/// A type of handshake message that QUIC carries in CRYPTO frames, so that
/// an endpoint receives it and limits its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum ReceivedMessageType {
    ClientHello,
    ServerHello,
    EncryptedExtensions,
    CertificateRequest,
    Certificate,
    CertificateVerify,
    Finished,
    NewSessionTicket,
}

impl ReceivedMessageType {
    /// The message type `message`, or `None` for a type QUIC does not carry.
    pub const fn new(message: HandshakeType) -> Option<Self> {
        Some(match message {
            HandshakeType::CLIENT_HELLO => Self::ClientHello,
            HandshakeType::SERVER_HELLO => Self::ServerHello,
            HandshakeType::ENCRYPTED_EXTENSIONS => Self::EncryptedExtensions,
            HandshakeType::CERTIFICATE_REQUEST => Self::CertificateRequest,
            HandshakeType::CERTIFICATE => Self::Certificate,
            HandshakeType::CERTIFICATE_VERIFY => Self::CertificateVerify,
            HandshakeType::FINISHED => Self::Finished,
            HandshakeType::NEW_SESSION_TICKET => Self::NewSessionTicket,
            _ => return None,
        })
    }

    /// The message type.
    pub const fn handshake_type(self) -> HandshakeType {
        match self {
            Self::ClientHello => HandshakeType::CLIENT_HELLO,
            Self::ServerHello => HandshakeType::SERVER_HELLO,
            Self::EncryptedExtensions => HandshakeType::ENCRYPTED_EXTENSIONS,
            Self::CertificateRequest => HandshakeType::CERTIFICATE_REQUEST,
            Self::Certificate => HandshakeType::CERTIFICATE,
            Self::CertificateVerify => HandshakeType::CERTIFICATE_VERIFY,
            Self::Finished => HandshakeType::FINISHED,
            Self::NewSessionTicket => HandshakeType::NEW_SESSION_TICKET,
        }
    }
}

/// One whole handshake message: its header and its body, in one contiguous
/// buffer.
///
/// The bytes go into the transcript as they are; the reader of the message
/// type runs on [`HandshakeMessage::encoded`], or on a slice of it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HandshakeMessage {
    encoded: Bytes,
}

impl HandshakeMessage {
    /// A message from its whole encoding, which the caller has written.
    pub(crate) fn from_encoded(encoded: Bytes) -> Self {
        debug_assert!(encoded.len() >= 4);
        Self { encoded }
    }

    /// The type of the message.
    pub fn handshake_type(&self) -> HandshakeType {
        HandshakeType(self.encoded[0])
    }

    /// The whole message, header included.
    pub fn encoded(&self) -> &Bytes {
        &self.encoded
    }

    /// Takes the whole message, header included.
    pub fn into_encoded(self) -> Bytes {
        self.encoded
    }
}

pub mod codec {
    //! Collecting a handshake message from the bytes of an encryption level
    //! as a `nitori_call` operation.
    //!
    //! [`read_handshake_message`] is also the operation struct
    //! [`ReadHandshakeMessage`], with `ReadHandshakeMessageExt` on receiver
    //! accesses and `ReadHandshakeMessageHostExt` on call executors.
    // `#[call]` generates constructors, fields and extension traits without
    // documentation comments; the module documentation above describes them.
    #![allow(missing_docs)]

    use super::{HandshakeMessage, HandshakeType, MessageLimits, ReceivedMessageType};
    use crate::alert::AlertDescription;
    use bytes::{BufMut, BytesMut};
    use nitori_call::{ReceiverAccess, call};
    use nitori_crypto::HashAlgorithm;
    use nitori_io::{
        Read,
        calls::{ReadArrayError, ReadArrayExt as _, ReadExactError, ReadExactExt as _},
    };
    use snafu::{IntoError, ResultExt, Snafu};

    /// Reads one handshake message: its type and length, then its body.
    ///
    /// The limit for the type comes from `limits`, and for Finished from
    /// `hash`, the hash of the cipher suite once it is known. A length above
    /// the limit fails before the body is read. The operation keeps the
    /// bytes it has read while it waits for more.
    ///
    /// # Errors
    ///
    /// [`ReadHandshakeMessageError`].
    #[call(sync)]
    pub async fn read_handshake_message<F: Read, A: ReceiverAccess<Family = F>>(
        io: &mut A,
        limits: MessageLimits,
        hash: Option<HashAlgorithm>,
    ) -> Result<HandshakeMessage, ReadHandshakeMessageError<F::Error>> {
        use read_handshake_message_error::*;
        let header = io.read_array::<4>().await.context(HeaderSnafu)?;
        let message_type = HandshakeType(header[0]);
        let len = u32::from_be_bytes([0, header[1], header[2], header[3]]);
        let Some(received) = ReceivedMessageType::new(message_type) else {
            return Err(UnexpectedMessageError { message_type }.into());
        };
        let Some(limit) = limits.limit(received, hash) else {
            return Err(UnexpectedMessageError { message_type }.into());
        };
        if len > limit {
            return Err(MessageTooLongError {
                message_type: received,
                len,
                limit,
            }
            .into());
        }
        let len = len as usize;
        let mut encoded = BytesMut::with_capacity(4 + len);
        encoded.put_slice(&header);
        match io.read_exact(&mut encoded, len).await {
            Ok(()) => {}
            Err(ReadExactError::Incomplete { source }) => {
                return Err(BodySnafu.into_error(ReadArrayError::Incomplete { source }));
            }
            Err(ReadExactError::Receiver { source, completed }) => {
                return Err(BodySnafu.into_error(ReadArrayError::Receiver { source, completed }));
            }
            // invariant: the buffer has room for the body, since it grows on
            // demand up to `isize::MAX` bytes.
            Err(ReadExactError::InsufficientCapacity { source }) => unreachable!("{source}"),
        }
        Ok(HandshakeMessage {
            encoded: encoded.freeze(),
        })
    }

    /// Failures that [`read_handshake_message`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_handshake_message_error), visibility(pub(crate)))]
    pub enum ReadHandshakeMessageError<E> {
        /// The type and length could not be read.
        #[snafu(display("handshake message header read failed"))]
        Header {
            /// The failure of reading the header.
            source: ReadArrayError<E>,
        },
        /// The message is of a type that is not received here.
        #[snafu(transparent)]
        Unexpected {
            /// The type.
            source: UnexpectedMessageError,
        },
        /// The message is longer than the limit for its type.
        #[snafu(transparent)]
        TooLong {
            /// The length and the limit.
            source: MessageTooLongError,
        },
        /// The body could not be read.
        #[snafu(display("handshake message body read failed"))]
        Body {
            /// The failure of reading the body.
            source: ReadArrayError<E>,
        },
    }

    impl<E> ReadHandshakeMessageError<E> {
        /// The alert the failure calls for: unexpected_message for a type
        /// that is not received, illegal_parameter for a message above its
        /// limit, decode_error for a stream that ends within a message and
        /// internal_error when the receiver fails.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::Unexpected { .. } => AlertDescription::UNEXPECTED_MESSAGE,
                Self::TooLong { .. } => AlertDescription::ILLEGAL_PARAMETER,
                Self::Header { source } | Self::Body { source } => match source {
                    ReadArrayError::Incomplete { .. } => AlertDescription::DECODE_ERROR,
                    ReadArrayError::Receiver { .. } => AlertDescription::INTERNAL_ERROR,
                },
            }
        }

        /// The receiver's error, if the failure came from the receiver.
        pub fn host_error(&self) -> Option<&E> {
            match self {
                Self::Header { source } | Self::Body { source } => source.host_error(),
                Self::Unexpected { .. } | Self::TooLong { .. } => None,
            }
        }
    }

    /// A message of a type that QUIC does not carry, or a Finished message
    /// before the cipher suite is known.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
    #[snafu(display("{message_type:?} message is not received here"))]
    pub struct UnexpectedMessageError {
        message_type: HandshakeType,
    }

    impl UnexpectedMessageError {
        /// The type of the message.
        pub const fn message_type(self) -> HandshakeType {
            self.message_type
        }
    }

    /// A message is longer than the limit for its type.
    ///
    /// It is constructed only when the check fails, so the length is always
    /// above the limit.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
    #[snafu(display("{message_type:?} message of {len} bytes exceeds the limit of {limit} bytes"))]
    pub struct MessageTooLongError {
        message_type: ReceivedMessageType,
        len: u32,
        limit: u32,
    }

    impl MessageTooLongError {
        /// The type of the message.
        pub const fn message_type(self) -> ReceivedMessageType {
            self.message_type
        }

        /// The length of the body that the header states.
        pub const fn length(self) -> u32 {
            self.len
        }

        /// The limit on the body.
        pub const fn limit(self) -> u32 {
            self.limit
        }
    }
}

#[cfg(test)]
mod tests;

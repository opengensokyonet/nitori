//! What the server's and the client's handshake engines share: the CRYPTO
//! byte streams they read and write, and how a handshake fails.
//!
//! A handshake engine runs in steps. Each role has a trait of its steps,
//! [`ServerHandshakeSteps`](crate::server::engine::ServerHandshakeSteps)
//! and [`ClientHandshakeSteps`](crate::client::engine::ClientHandshakeSteps),
//! which the engine's start state implements. A step is an async function
//! that takes the state before it by value and runs until the connection
//! has something to do: it returns what the connection installs, such as
//! traffic secrets or the completion of the handshake, together with the
//! state of the next step. What the engine owns for the whole handshake,
//! such as the backend and the capabilities of its role, is in its states;
//! what the connection provides, a step receives as one access, lent on
//! every poll, whose family has the role's capability:
//!
//! - [`CryptoInput`]: the CRYPTO bytes the peer sent, at the current
//!   [`CryptoLevel`];
//! - [`CryptoOutput`]: the CRYPTO bytes to send, at the current level;
//! - the transport parameters, through synchronous methods of the role's
//!   own capability.
//!
//! The engine reads and writes the bytes of the current level only, and
//! moves each stream to the next level as a separate step (RFC 9001
//! §4.1.3). A step that fails ends the handshake with a
//! [`HandshakeError`]: with an alert, with a [`ProtocolViolationError`] for
//! the connection to close with PROTOCOL_VIOLATION, or with the
//! connection's own error.
//!
//! `#[capability]` generates `{Trait}Ext` and `{Trait}ViewExt` for each
//! receiver capability, with the methods the trait documents.

// The generated extension traits have no documentation comments; the
// module documentation above describes them.
#![allow(missing_docs)]

use crate::{
    alert::{AlertDescription, CallsForAlert},
    extension::early_data::MaxEarlyDataSize,
    message::{LegacySessionId, codec::ReadHandshakeMessageError},
};
use core::{convert::Infallible, pin::Pin};
use nitori_call::capability;
use nitori_io::{
    Read, Write,
    calls::{ReadArrayError, WriteAllError},
    error::WriteZeroError,
};
use snafu::Snafu;

/// An encryption level that carries CRYPTO frames: Initial, Handshake or
/// 1-RTT. 0-RTT packets carry none (RFC 9000 §12.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CryptoLevel {
    /// Initial.
    Initial,
    /// Handshake.
    Handshake,
    /// 1-RTT.
    OneRtt,
}

/// The CRYPTO bytes a handshake engine receives: those of the current
/// level, read with [`Read`].
///
/// Reading waits for bytes that have not arrived. A level begins as
/// [`CryptoLevel::Initial`] and moves forward with
/// [`CryptoInput::move_input_to`].
#[capability]
pub trait CryptoInput: Read {
    /// A failure to move to a level.
    type LevelError;

    /// Moves to `level`: later reads return its bytes.
    ///
    /// # Errors
    ///
    /// As the implementation documents; RFC 9001 §4.1.3 makes it an error
    /// when bytes of the level being left remain unread.
    fn move_input_to<'v>(
        view: Pin<&mut Self::ReceiverView<'v>>,
        level: CryptoLevel,
    ) -> Result<(), Self::LevelError>
    where
        Self: 'v;
}

/// The CRYPTO bytes a handshake engine sends: those of the current level,
/// written with [`Write`].
///
/// A level begins as [`CryptoLevel::Initial`] and moves forward with
/// [`CryptoOutput::move_output_to`]; what was written before stays at its
/// level.
#[capability]
pub trait CryptoOutput: Write {
    /// Moves to `level`: later writes go to it.
    fn move_output_to<'v>(view: Pin<&mut Self::ReceiverView<'v>>, level: CryptoLevel)
    where
        Self: 'v;
}

/// How a handshake fails: with an alert, with a violation of QUIC's rules
/// for TLS, or with a failure of the connection's receivers.
///
/// `F` is the role's failure, which names its alert; in QUIC the alert
/// closes the connection with the error code `0x0100` plus the alert
/// description (RFC 9001 §4.8). `E` is the error of the connection, which
/// the receivers report, such as CRYPTO_BUFFER_EXCEEDED or
/// TRANSPORT_PARAMETER_ERROR.
#[derive(Debug, Snafu)]
pub enum HandshakeError<F, E> {
    /// The handshake ends with the alert `source` calls for.
    #[snafu(display("handshake failed"))]
    Alert {
        /// The failure.
        source: F,
    },
    /// The peer broke a rule RFC 9001 sets for TLS in QUIC: the connection
    /// closes with PROTOCOL_VIOLATION.
    #[snafu(display("peer violated the QUIC rules for TLS"))]
    ProtocolViolation {
        /// The rule.
        source: ProtocolViolationError,
    },
    /// A receiver of the connection failed.
    #[snafu(display("connection failed during the handshake"))]
    Transport {
        /// The connection's error.
        source: E,
    },
}

impl<F: CallsForAlert, E> HandshakeError<F, E> {
    /// The alert to close with, or `None` when the connection closes with a
    /// transport error instead.
    pub fn alert(&self) -> Option<AlertDescription> {
        match self {
            Self::Alert { source } => Some(source.alert()),
            Self::ProtocolViolation { .. } | Self::Transport { .. } => None,
        }
    }
}

impl<F, E> HandshakeError<F, E> {
    /// The role's failure, if the handshake ends with an alert.
    pub fn failure(&self) -> Option<&F> {
        match self {
            Self::Alert { source } => Some(source),
            Self::ProtocolViolation { .. } | Self::Transport { .. } => None,
        }
    }

    /// The rule the peer broke, if the connection closes with
    /// PROTOCOL_VIOLATION.
    pub fn protocol_violation(&self) -> Option<&ProtocolViolationError> {
        match self {
            Self::ProtocolViolation { source } => Some(source),
            Self::Alert { .. } | Self::Transport { .. } => None,
        }
    }

    /// The connection's error, if a receiver failed.
    pub fn transport(&self) -> Option<&E> {
        match self {
            Self::Transport { source } => Some(source),
            Self::Alert { .. } | Self::ProtocolViolation { .. } => None,
        }
    }

    /// Converts the role's failure with `failure` and the connection's error
    /// with `transport`; a broken rule stays as it is.
    ///
    /// A failure of an operation that runs before the steps, such as reading
    /// the first ClientHello, converts this way into the failure of the
    /// steps.
    pub fn map<G, D>(
        self,
        failure: impl FnOnce(F) -> G,
        transport: impl FnOnce(E) -> D,
    ) -> HandshakeError<G, D> {
        match self {
            Self::Alert { source } => HandshakeError::Alert {
                source: failure(source),
            },
            Self::ProtocolViolation { source } => HandshakeError::ProtocolViolation { source },
            Self::Transport { source } => HandshakeError::Transport {
                source: transport(source),
            },
        }
    }
}

/// A rule of RFC 9001 for TLS in QUIC that the peer broke, which the
/// connection closes with PROTOCOL_VIOLATION.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
pub enum ProtocolViolationError {
    /// A NewSessionTicket allows 0-RTT with another max_early_data_size than
    /// 0xffffffff (RFC 9001 §4.6.1, MUST).
    #[snafu(transparent)]
    MaxEarlyDataSize {
        /// The size.
        source: MaxEarlyDataSizeError,
    },
    /// A ClientHello has a non-empty legacy_session_id, which asks for the
    /// middlebox compatibility mode (RFC 9001 §8.4, SHOULD).
    #[snafu(transparent)]
    LegacySessionId {
        /// The session ID.
        source: LegacySessionIdError,
    },
}

/// The max_early_data_size of a NewSessionTicket is not 0xffffffff.
///
/// It is constructed only when the check fails, so the size is never
/// [`MaxEarlyDataSize::QUIC`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("max_early_data_size {} in QUIC", size.get()))]
pub struct MaxEarlyDataSizeError {
    size: MaxEarlyDataSize,
}

impl MaxEarlyDataSizeError {
    /// Checks that `size` is [`MaxEarlyDataSize::QUIC`].
    pub(crate) fn check(size: MaxEarlyDataSize) -> Result<(), Self> {
        if size == MaxEarlyDataSize::QUIC {
            return Ok(());
        }
        Err(Self { size })
    }

    /// The size the ticket stated.
    pub const fn size(self) -> MaxEarlyDataSize {
        self.size
    }
}

/// The legacy_session_id of a ClientHello is not empty.
///
/// It is constructed only when the check fails, so the session ID is never
/// empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("ClientHello legacy_session_id of {} bytes in QUIC", session_id.as_bytes().len()))]
pub struct LegacySessionIdError {
    session_id: LegacySessionId,
}

impl LegacySessionIdError {
    /// Checks that `session_id` is empty.
    pub(crate) fn check(session_id: LegacySessionId) -> Result<(), Self> {
        if session_id == LegacySessionId::EMPTY {
            return Ok(());
        }
        Err(Self { session_id })
    }

    /// The session ID the ClientHello carried.
    pub const fn session_id(&self) -> &LegacySessionId {
        &self.session_id
    }
}

/// Splits a failure to read a handshake message into the part that calls
/// for an alert and the receiver's own failure.
pub(crate) fn split_read<E>(
    error: ReadHandshakeMessageError<E>,
) -> Result<ReadHandshakeMessageError<Infallible>, E> {
    fn array<E>(error: ReadArrayError<E>) -> Result<ReadArrayError<Infallible>, E> {
        match error {
            ReadArrayError::Incomplete { source } => Ok(ReadArrayError::Incomplete { source }),
            ReadArrayError::Receiver { source, .. } => Err(source),
        }
    }
    Ok(match error {
        ReadHandshakeMessageError::Header { source } => ReadHandshakeMessageError::Header {
            source: array(source)?,
        },
        ReadHandshakeMessageError::Body { source } => ReadHandshakeMessageError::Body {
            source: array(source)?,
        },
        ReadHandshakeMessageError::Unexpected { source } => {
            ReadHandshakeMessageError::Unexpected { source }
        }
        ReadHandshakeMessageError::TooLong { source } => {
            ReadHandshakeMessageError::TooLong { source }
        }
    })
}

/// Splits a failure to write into the sink that stopped and the receiver's
/// own failure.
pub(crate) fn split_write<E>(error: WriteAllError<E>) -> Result<WriteZeroError, E> {
    match error {
        WriteAllError::WriteZero { source } => Ok(source),
        WriteAllError::Receiver { source, .. } => Err(source),
    }
}

/// Encodes a message with a synchronous writer on a vector.
pub(crate) fn encode(len: usize, write: impl FnOnce(&mut Vec<u8>)) -> bytes::Bytes {
    let mut encoded = Vec::with_capacity(len);
    write(&mut encoded);
    debug_assert_eq!(encoded.len(), len);
    bytes::Bytes::from(encoded)
}

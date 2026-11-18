//! The server's handshake engine.
//!
//! [`ServerHandshake`] is the start state of a TLS 1.3 handshake for a QUIC
//! server, from the ClientHello to the session tickets, and implements
//! [`ServerHandshakeSteps`]. It owns the backend and the server's
//! capabilities. Each step receives the connection as one access, lent on
//! every poll, whose family has the [`ServerHandshakeCapability`] (see
//! [`engine`](crate::engine)): the CRYPTO streams and a [`ServerTransport`],
//! whose [`ServerTransport::negotiate`] checks the client's transport
//! parameters and gives the server's own.
//!
//! The first ClientHello is read before the engine runs, by
//! [`read_first_client_hello`]: it needs only the limits on messages, not
//! the engine's state, so a server can read it, and decide what to do with
//! the connection, before it builds the engine. The steps go:
//!
//! 1. [`write_server_hello`](ServerHandshakeSteps::write_server_hello)
//!    takes the [`FirstClientHello`], opens its layers, selects the target
//!    and chooses the cipher suite and group; when the client sent no key
//!    share of that group, it writes a HelloRetryRequest and reads the
//!    second ClientHello, whose layers it opens with what opening the
//!    first gave. It opens an offered ticket and checks its binder,
//!    negotiates the transport parameters, and admits 0-RTT only when the
//!    handshake, the connection and the ticket policy all agree. It writes
//!    the ServerHello and returns [`ServerHelloWritten`], with the selected
//!    target and application protocol and the 0-RTT secret when it
//!    admitted 0-RTT.
//! 2. [`write_server_finished`](ServerHandshakeSteps::write_server_finished)
//!    moves both streams to Handshake, writes EncryptedExtensions and,
//!    unless it resumed, Certificate and CertificateVerify, whose signature
//!    may wait; it writes its Finished and returns the
//!    [`OneRttSendTrafficSecret`].
//! 3. [`read_client_finished`](ServerHandshakeSteps::read_client_finished)
//!    reads the client's Finished: the handshake is complete, and it returns
//!    [`ServerHandshakeCompleted`] with the exporter and the client's 1-RTT
//!    secret, which it held back until now (RFC 9001 §5.7).
//! 4. [`write_tickets`](ServerHandshakeSteps::write_tickets) moves both
//!    streams to 1-RTT, issues the session tickets of its ticket policy, and
//!    ends; it issues none when the client did not name psk_dhe_ke
//!    (RFC 9846 §4.3.9).
//!
//! A failure ends the handshake with a [`HandshakeError`]; a ClientHello
//! with a non-empty legacy_session_id ends it with
//! [`HandshakeError::ProtocolViolation`] (RFC 9001 §8.4), the first one
//! already in [`read_first_client_hello`].
//!
//! This version neither opens encrypted layers nor asks for a client
//! certificate: it takes layers whose confirmation and retry
//! configurations, and a peer authentication whose request, are
//! [`Infallible`], so those steps have no values to act on.
//!
//! [`read_first_client_hello`] is a `nitori_call` operation, and `#[call]`
//! generates [`ReadFirstClientHello`] and its extension trait.
//! `#[call]` cannot be written on the methods of a trait. Each step is also
//! a `nitori_call` operation for every [`ServerHandshakeSteps`], through
//! [`write_server_hello_step`], [`write_server_finished_step`],
//! [`read_client_finished_step`] and [`write_tickets_step`]: `#[call]`
//! generates their operation structs ([`WriteServerHelloStep`] and so on),
//! each with a `new` constructor taking the state, and their extension
//! traits. `#[capability]` generates `ServerTransportExt` and
//! `ServerTransportViewExt`.

// The generated items have no documentation comments; the module
// documentation above describes them.
#![allow(missing_docs)]

use crate::{
    alert::{AlertDescription, CallsForAlert},
    cipher_suite::SupportedSuite,
    clock::{Clock, elapsed},
    crypto::TlsCrypto,
    engine::{
        CryptoInput, CryptoInputExt as _, CryptoLevel, CryptoOutput, CryptoOutputExt as _,
        HandshakeError, LegacySessionIdError, encode, split_read, split_write,
    },
    extension::{
        InvalidExtensionBlockError, OpaqueData,
        early_data::MaxEarlyDataSize,
        key_share::KeyShareEntry,
        pre_shared_key::{OfferedPsks, PskIndex},
        psk_key_exchange_modes::PskKeyExchangeMode,
    },
    key_log::{KeyLog, KeyLogLabel},
    message::{
        HandshakeMessage, HelloRandom, MessageLimits,
        certificate::{
            Certificate, CertificateEntry, CertificateList, InvalidCertificateEntryError,
            InvalidCertificateError, codec::write_certificate,
        },
        certificate_verify::{CertificateVerify, codec::write_certificate_verify},
        client_hello::{
            ClientHello, ClientHelloExtension,
            codec::{ReadClientHelloError, read_client_hello},
        },
        codec::{ReadHandshakeMessageError, ReadHandshakeMessageExt as _},
        encrypted_extensions::{
            EncryptedExtensions, EncryptedExtensionsExtension, codec::write_encrypted_extensions,
        },
        new_session_ticket::{
            NewSessionTicket, NewSessionTicketExtension, TicketNonce,
            codec::write_new_session_ticket,
        },
        server_hello::{
            HelloRetryRequest, HelloRetryRequestExtension, ServerHello, ServerHelloExtension,
            codec::{write_hello_retry_request, write_server_hello},
        },
    },
    role::Server,
    schedule::{
        BinderMismatchError, EarlySecret, Exporter, FinishedError, HandshakeSecret,
        HandshakeTrafficSecrets, MainSecret, OneRttReceiveTrafficSecret, OneRttSendTrafficSecret,
        PskEarlySecret, ResumptionPsk, ResumptionSecret, Transcript, ZeroRttReceiveTrafficSecret,
    },
    server::{
        choice::{ClientOffer, NoCommonParametersError, ParameterChoice},
        identity::{CertificateSelection, NoCommonSchemeError},
        layers::{AcceptedHello, HelloLayers},
        peer::PeerAuthentication,
        targets::{SelectedTarget, TargetSelection},
        ticket::{
            ClientHelloKey, Issuing, Opened, Resumable, TicketPolicy, TicketSlot, TicketState,
        },
    },
    vector::LengthOutOfRangeError,
    version::ProtocolVersion,
};
use bytes::Bytes;
use core::{convert::Infallible, pin::Pin, time::Duration};
use nitori_call::{LentAccess, ReceiverAccess, ReceiverFamily, call, capability};
use nitori_crypto::AeadAlgorithm;
use nitori_crypto::{
    Hash, HashAlgorithm, KeyExchange, Random, RandomError, RunningHash, Secret, ServerShareError,
    SigningKey,
};
use nitori_io::{Read, Write, calls::WriteAllExt as _, error::WriteZeroError};
use snafu::Snafu;
use time::UtcDateTime;

/// The connection's side of the transport parameters, as a server's
/// handshake asks for it.
///
/// `T` is the type of the server's selected target.
#[capability]
pub trait ServerTransport<T>: ReceiverFamily {
    /// A failure, such as TRANSPORT_PARAMETER_ERROR.
    type Error;

    /// Checks `peer`, the client's `quic_transport_parameters`, records
    /// what the connection learns from them, and gives the server's own,
    /// for `selected`, the target and application protocol. `resumed` is
    /// what the ticket the server resumes with recorded, against which the
    /// connection decides whether 0-RTT is compatible (RFC 9000 §7.4.1,
    /// RFC 9001 §4.6.3).
    ///
    /// # Errors
    ///
    /// As the implementation documents.
    fn negotiate<'v>(
        view: Pin<&mut Self::ReceiverView<'v>>,
        peer: &[u8],
        selected: &SelectedTarget<T>,
        resumed: Option<&TicketState<T>>,
    ) -> Result<Negotiated, Self::Error>
    where
        Self: 'v;
}

/// What the connection answers to [`ServerTransport::negotiate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Negotiated {
    /// The server's own transport parameters, for EncryptedExtensions.
    pub local: OpaqueData<Bytes>,
    /// Whether the connection accepts 0-RTT with the ticket: its transport
    /// parameters and the application's configuration are compatible with
    /// what the ticket recorded, and `local` does not lower the limits
    /// 0-RTT relies on. Without a ticket it is not read.
    pub early_data: bool,
}

/// The fixed choices of a server's handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerHandshakeSettings {
    /// The limits on the messages the server receives after the first
    /// ClientHello, which [`read_first_client_hello`] reads with limits of
    /// its own.
    pub limits: MessageLimits,
    /// How far the age a client states for its ticket may be from the age
    /// the server measures for 0-RTT to be admitted (RFC 9846 §8.3); 10
    /// seconds by default.
    pub ticket_age_tolerance: Duration,
}

impl Default for ServerHandshakeSettings {
    fn default() -> Self {
        Self {
            limits: MessageLimits::default(),
            ticket_age_tolerance: Duration::from_secs(10),
        }
    }
}

/// Everything a server's handshake asks of the connection, on one family:
/// the CRYPTO bytes it reads and writes, the moves of their levels, and the
/// transport parameters.
///
/// `T` is the type of the server's selected target. It is implemented for
/// every family with [`CryptoInput`], [`CryptoOutput`] and
/// [`ServerTransport`] whose failures to read, to move the input's level and
/// to write convert into the error of [`ServerTransport`]; the three
/// conversions are its methods.
pub trait ServerHandshakeCapability<T>: CryptoInput + CryptoOutput + ServerTransport<T> {
    /// Converts a failure to read CRYPTO bytes.
    fn convert_read_error(error: <Self as Read>::Error) -> <Self as ServerTransport<T>>::Error;

    /// Converts a failure to move the input to a level.
    fn convert_level_error(
        error: <Self as CryptoInput>::LevelError,
    ) -> <Self as ServerTransport<T>>::Error;

    /// Converts a failure to write CRYPTO bytes.
    fn convert_write_error(error: <Self as Write>::Error) -> <Self as ServerTransport<T>>::Error;
}

impl<T, W> ServerHandshakeCapability<T> for W
where
    W: CryptoInput + CryptoOutput + ServerTransport<T>,
    <W as Read>::Error: Into<<W as ServerTransport<T>>::Error>,
    <W as CryptoInput>::LevelError: Into<<W as ServerTransport<T>>::Error>,
    <W as Write>::Error: Into<<W as ServerTransport<T>>::Error>,
{
    fn convert_read_error(error: <W as Read>::Error) -> <W as ServerTransport<T>>::Error {
        error.into()
    }

    fn convert_level_error(
        error: <W as CryptoInput>::LevelError,
    ) -> <W as ServerTransport<T>>::Error {
        error.into()
    }

    fn convert_write_error(error: <W as Write>::Error) -> <W as ServerTransport<T>>::Error {
        error.into()
    }
}

/// The first ClientHello of a connection, read at Initial by
/// [`read_first_client_hello`]: the message as it arrived and the
/// ClientHello decoded from it.
///
/// Its legacy_session_id is empty, as QUIC requires (RFC 9001 §8.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstClientHello {
    encoded: HandshakeMessage,
    client_hello: ClientHello<Bytes>,
}

impl FirstClientHello {
    /// The message as it arrived, which goes into the transcript.
    pub const fn encoded(&self) -> &HandshakeMessage {
        &self.encoded
    }

    /// The ClientHello decoded from [`FirstClientHello::encoded`].
    pub const fn client_hello(&self) -> &ClientHello<Bytes> {
        &self.client_hello
    }

    /// The message and the ClientHello decoded from it.
    pub fn into_parts(self) -> (HandshakeMessage, ClientHello<Bytes>) {
        (self.encoded, self.client_hello)
    }
}

/// How reading the first ClientHello fails with an alert.
#[derive(Debug, Snafu)]
pub enum ReadFirstClientHelloError {
    /// The handshake message could not be collected.
    #[snafu(display("handshake message read failed"))]
    Message {
        /// The failure.
        source: ReadHandshakeMessageError<Infallible>,
    },
    /// The message is not a ClientHello that could be read.
    #[snafu(display("ClientHello read failed"))]
    ClientHello {
        /// The failure.
        source: ReadClientHelloError,
    },
}

impl CallsForAlert for ReadFirstClientHelloError {
    /// The alert of the source.
    fn alert(&self) -> AlertDescription {
        match self {
            Self::Message { source } => source.alert(),
            Self::ClientHello { source } => source.alert(),
        }
    }
}

/// Reads the first ClientHello of a connection from the CRYPTO bytes at
/// Initial, within `limits`.
///
/// It needs nothing of the engine, so a server can read the ClientHello
/// before it builds the engine, and hand it to
/// [`ServerHandshakeSteps::write_server_hello`]. A ClientHello sent again
/// after a HelloRetryRequest is read by that step.
///
/// # Errors
///
/// [`HandshakeError::Alert`] when the message cannot be collected or is not
/// a ClientHello, [`HandshakeError::ProtocolViolation`] when its
/// legacy_session_id is not empty, and [`HandshakeError::Transport`] with
/// the failure of the input.
#[call]
pub async fn read_first_client_hello<W: CryptoInput>(
    host: &mut impl LentAccess<Family = W>,
    limits: MessageLimits,
) -> Result<FirstClientHello, HandshakeError<ReadFirstClientHelloError, <W as Read>::Error>> {
    let encoded =
        host.read_handshake_message(limits, None)
            .await
            .map_err(|error| match split_read(error) {
                Ok(source) => alert(ReadFirstClientHelloError::Message { source }),
                Err(source) => HandshakeError::Transport { source },
            })?;
    let client_hello = read_client_hello(&mut encoded.encoded().clone())
        .map_err(|source| alert(ReadFirstClientHelloError::ClientHello { source }))?;
    check_session_id(&client_hello)?;
    Ok(FirstClientHello {
        encoded,
        client_hello,
    })
}

/// The steps of a server's handshake, implemented by the engine's start
/// state.
///
/// Each step takes the state the step before returned by value, with the
/// connection as an access whose family `W` has the
/// [`ServerHandshakeCapability`]; it runs until the connection has
/// something to install and returns that with the state of the next step.
/// The family is a parameter of each step, so no state depends on it. Every
/// step fails with the same [`HandshakeError`].
///
/// The steps are async functions of the trait, so code generic over the
/// trait cannot name their futures, nor require that they be `Send`.
#[allow(async_fn_in_trait)]
pub trait ServerHandshakeSteps: Sized {
    /// The backend, which the exporter keeps.
    type Crypto: TlsCrypto;

    /// The type of the selected target.
    type Target;

    /// The server's failure, which names its alert; a failure to read the
    /// first ClientHello converts into it.
    type AlertError: CallsForAlert + From<ReadFirstClientHelloError>;

    /// The state after the ServerHello is written.
    type ServerHelloWrittenState;

    /// The state after the server's Finished is written.
    type ServerFinishedWrittenState;

    /// The state after the client's Finished is verified.
    type ServerHandshakeCompletedState;

    /// Continues from `first`, reads the second ClientHello after a
    /// HelloRetryRequest when the server sends one, negotiates the transport
    /// parameters and writes the ServerHello.
    ///
    /// # Errors
    ///
    /// As [`HandshakeError`] describes.
    async fn write_server_hello<W: ServerHandshakeCapability<Self::Target>>(
        self,
        first: FirstClientHello,
        host: &mut impl LentAccess<Family = W>,
    ) -> Result<
        (
            ServerHelloWritten<Self::Target>,
            Self::ServerHelloWrittenState,
        ),
        StepError<Self, W>,
    >;

    /// Moves both streams to Handshake and writes the server's flight up to
    /// its Finished.
    ///
    /// # Errors
    ///
    /// As [`HandshakeError`] describes.
    async fn write_server_finished<W: ServerHandshakeCapability<Self::Target>>(
        state: Self::ServerHelloWrittenState,
        host: &mut impl LentAccess<Family = W>,
    ) -> Result<(OneRttSendTrafficSecret, Self::ServerFinishedWrittenState), StepError<Self, W>>;

    /// Reads and verifies the client's Finished.
    ///
    /// # Errors
    ///
    /// As [`HandshakeError`] describes.
    #[allow(clippy::type_complexity)]
    async fn read_client_finished<W: ServerHandshakeCapability<Self::Target>>(
        state: Self::ServerFinishedWrittenState,
        host: &mut impl LentAccess<Family = W>,
    ) -> Result<
        (
            ServerHandshakeCompleted<Self::Crypto, Self::Target>,
            Self::ServerHandshakeCompletedState,
        ),
        StepError<Self, W>,
    >;

    /// Moves both streams to 1-RTT and writes the session tickets.
    ///
    /// # Errors
    ///
    /// As [`HandshakeError`] describes.
    async fn write_tickets<W: ServerHandshakeCapability<Self::Target>>(
        state: Self::ServerHandshakeCompletedState,
        host: &mut impl LentAccess<Family = W>,
    ) -> Result<(), StepError<Self, W>>;
}

/// How a step of `E` fails on the family `W`.
type StepError<E, W> = HandshakeError<
    <E as ServerHandshakeSteps>::AlertError,
    <W as ServerTransport<<E as ServerHandshakeSteps>::Target>>::Error,
>;

/// What [`ServerHandshakeSteps::write_server_hello`] returns: the Handshake
/// traffic secrets, the selected target with its application protocol and,
/// when the server admitted 0-RTT, the secret to receive 0-RTT with.
///
/// `T` is the type of the selected target.
#[doc(alias = "HandshakeTrafficSecrets")]
pub struct ServerHelloWritten<T> {
    /// The AEAD of the cipher suite, for every secret of the handshake.
    pub aead: AeadAlgorithm,
    /// The Handshake traffic secrets.
    pub secrets: HandshakeTrafficSecrets,
    /// The selected target and application protocol, which hold for the
    /// rest of the handshake.
    pub selected: SelectedTarget<T>,
    /// The 0-RTT secret, `None` when 0-RTT is not admitted.
    pub zero_rtt: Option<ZeroRttReceiveTrafficSecret>,
}

/// What [`ServerHandshakeSteps::read_client_finished`] returns: the client's
/// Finished is verified, and the handshake is complete.
///
/// `C` is the backend and `T` the type of the selected target.
pub struct ServerHandshakeCompleted<C: TlsCrypto, T> {
    /// The client's 1-RTT secret, the server's to receive with.
    pub one_rtt_receive: OneRttReceiveTrafficSecret,
    /// The exporter of the connection.
    pub exporter: Exporter<C>,
    /// The selected target and application protocol.
    pub selected: SelectedTarget<T>,
    /// Whether the server authenticated with the PSK of a ticket.
    pub resumed: bool,
}

/// A step of a server's handshake as an operation: see
/// [`ServerHandshakeSteps::write_server_hello`].
///
/// # Errors
///
/// As the step's.
#[call]
#[allow(clippy::type_complexity)]
pub async fn write_server_hello_step<W, E>(
    host: &mut impl LentAccess<Family = W>,
    tls: E,
    first: FirstClientHello,
) -> Result<
    (
        ServerHelloWritten<<E as ServerHandshakeSteps>::Target>,
        <E as ServerHandshakeSteps>::ServerHelloWrittenState,
    ),
    StepError<E, W>,
>
where
    E: ServerHandshakeSteps,
    W: ServerHandshakeCapability<<E as ServerHandshakeSteps>::Target>,
{
    tls.write_server_hello(first, host).await
}

/// A step of a server's handshake as an operation: see
/// [`ServerHandshakeSteps::write_server_finished`].
///
/// # Errors
///
/// As the step's.
#[call]
#[allow(clippy::type_complexity)]
pub async fn write_server_finished_step<W, E>(
    host: &mut impl LentAccess<Family = W>,
    state: <E as ServerHandshakeSteps>::ServerHelloWrittenState,
) -> Result<
    (
        OneRttSendTrafficSecret,
        <E as ServerHandshakeSteps>::ServerFinishedWrittenState,
    ),
    StepError<E, W>,
>
where
    E: ServerHandshakeSteps,
    W: ServerHandshakeCapability<<E as ServerHandshakeSteps>::Target>,
{
    E::write_server_finished(state, host).await
}

/// A step of a server's handshake as an operation: see
/// [`ServerHandshakeSteps::read_client_finished`].
///
/// # Errors
///
/// As the step's.
#[call]
#[allow(clippy::type_complexity)]
pub async fn read_client_finished_step<W, E>(
    host: &mut impl LentAccess<Family = W>,
    state: <E as ServerHandshakeSteps>::ServerFinishedWrittenState,
) -> Result<
    (
        ServerHandshakeCompleted<
            <E as ServerHandshakeSteps>::Crypto,
            <E as ServerHandshakeSteps>::Target,
        >,
        <E as ServerHandshakeSteps>::ServerHandshakeCompletedState,
    ),
    StepError<E, W>,
>
where
    E: ServerHandshakeSteps,
    W: ServerHandshakeCapability<<E as ServerHandshakeSteps>::Target>,
{
    E::read_client_finished(state, host).await
}

/// A step of a server's handshake as an operation: see
/// [`ServerHandshakeSteps::write_tickets`].
///
/// # Errors
///
/// As the step's.
#[call]
pub async fn write_tickets_step<W, E>(
    host: &mut impl LentAccess<Family = W>,
    state: <E as ServerHandshakeSteps>::ServerHandshakeCompletedState,
) -> Result<(), StepError<E, W>>
where
    E: ServerHandshakeSteps,
    W: ServerHandshakeCapability<<E as ServerHandshakeSteps>::Target>,
{
    E::write_tickets(state, host).await
}

/// A required extension a ClientHello lacks (RFC 9846 §9.2, RFC 9001
/// §8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RequiredExtension {
    /// `supported_groups`: the server uses (EC)DHE in every handshake.
    SupportedGroups,
    /// `key_share`, which goes with `supported_groups`.
    KeyShare,
    /// `signature_algorithms`, for a server that authenticates with a
    /// certificate.
    SignatureAlgorithms,
    /// `psk_key_exchange_modes`, which goes with `pre_shared_key`.
    PskKeyExchangeModes,
    /// `quic_transport_parameters`.
    QuicTransportParameters,
}

/// How a server's handshake fails with an alert.
///
/// `L`, `S`, `G` and `K` are the failures of opening the layers, selecting
/// the target, signing and sealing a ticket.
#[derive(Debug, Snafu)]
#[snafu(module(server_alert_error), visibility(pub(crate)))]
pub enum ServerAlertError<L, S, G, K> {
    /// A handshake message could not be collected.
    #[snafu(display("handshake message read failed"))]
    Message {
        /// The failure.
        source: ReadHandshakeMessageError<Infallible>,
    },
    /// The ClientHello could not be read.
    #[snafu(display("ClientHello read failed"))]
    ClientHello {
        /// The failure.
        source: ReadClientHelloError,
    },
    /// The ClientHello does not offer TLS 1.3 in `supported_versions`.
    #[snafu(display("ClientHello does not offer TLS 1.3"))]
    UnsupportedVersion,
    /// The ClientHello lacks a required extension.
    #[snafu(display("ClientHello lacks {extension:?}"))]
    MissingExtension {
        /// The extension.
        extension: RequiredExtension,
    },
    /// The layers of the ClientHello could not be opened.
    #[snafu(display("ClientHello layers could not be opened"))]
    Layers {
        /// The failure.
        source: L,
    },
    /// No target could be selected.
    #[snafu(display("target selection failed"))]
    Target {
        /// The failure.
        source: S,
    },
    /// The ClientHello and the server have no suite or no group in common.
    #[snafu(display("no common parameters"))]
    NoCommonParameters {
        /// Which.
        source: NoCommonParametersError,
    },
    /// The parameter choice picked a suite or group the client did not
    /// offer.
    #[snafu(display("parameter choice outside the client's offer"))]
    ChoiceNotOffered,
    /// The second ClientHello does not carry exactly one key share, of the
    /// group the HelloRetryRequest named (RFC 9846 §4.2.2).
    #[snafu(display("second ClientHello does not share the requested group"))]
    RetryKeyShare,
    /// The second ClientHello no longer offers the suite or the group the
    /// HelloRetryRequest named.
    #[snafu(display("second ClientHello withdrew the chosen suite or group"))]
    RetryOffer,
    /// The ticket policy named an identity the ClientHello does not have.
    #[snafu(display("ticket policy named PSK {}, which was not offered", index.get()))]
    TicketIndex {
        /// The index.
        index: PskIndex,
    },
    /// The binder of the PSK does not match.
    #[snafu(display("PSK binder rejected"))]
    Binder {
        /// The failure.
        source: BinderMismatchError,
    },
    /// The key exchange failed.
    #[snafu(display("key exchange failed"))]
    KeyExchange {
        /// The failure.
        source: ServerShareError,
    },
    /// The backend gave no random bytes.
    #[snafu(display("random generation failed"))]
    Random {
        /// The failure.
        source: RandomError,
    },
    /// EncryptedExtensions does not fit its block.
    #[snafu(display("EncryptedExtensions too long"))]
    EncryptedExtensions {
        /// The failure.
        source: InvalidExtensionBlockError,
    },
    /// The key signs with none of the client's schemes.
    #[snafu(display("no common signature scheme"))]
    NoCommonScheme {
        /// The failure.
        source: NoCommonSchemeError,
    },
    /// A certificate of the chain cannot be sent.
    #[snafu(display("certificate of the chain cannot be sent"))]
    CertificateEntry {
        /// The failure.
        source: InvalidCertificateEntryError,
    },
    /// The chain does not fit a Certificate message.
    #[snafu(display("certificate chain too long"))]
    Certificate {
        /// The failure.
        source: InvalidCertificateError,
    },
    /// The signature of CertificateVerify could not be made.
    #[snafu(display("CertificateVerify signing failed"))]
    Signing {
        /// The failure.
        source: G,
    },
    /// The signature does not fit CertificateVerify.
    #[snafu(display("signature length out of range"))]
    SignatureLength {
        /// The failure.
        source: LengthOutOfRangeError,
    },
    /// The client's Finished is not valid.
    #[snafu(display("client Finished rejected"))]
    Finished {
        /// The failure.
        source: FinishedError,
    },
    /// A ticket could not be sealed.
    #[snafu(display("ticket sealing failed"))]
    Ticket {
        /// The failure.
        source: K,
    },
    /// The output stopped accepting bytes.
    #[snafu(display("CRYPTO output stopped"))]
    OutputStopped {
        /// The failure.
        source: WriteZeroError,
    },
}

impl<L: CallsForAlert, S: CallsForAlert, G, K> CallsForAlert for ServerAlertError<L, S, G, K> {
    /// The alert the failure calls for: protocol_version without TLS 1.3
    /// (RFC 9846 §4.3.1), missing_extension for a required extension
    /// (§9.2), illegal_parameter for a second ClientHello that does not
    /// follow the HelloRetryRequest (§4.1.2) and an invalid key share,
    /// decrypt_error for a binder (§4.2.11), handshake_failure without
    /// common parameters or scheme, internal_error for failures of the
    /// server itself, and that of the source otherwise.
    fn alert(&self) -> AlertDescription {
        match self {
            Self::Message { source } => source.alert(),
            Self::ClientHello { source } => source.alert(),
            Self::UnsupportedVersion => AlertDescription::PROTOCOL_VERSION,
            Self::MissingExtension { .. } => AlertDescription::MISSING_EXTENSION,
            Self::Layers { source } => source.alert(),
            Self::Target { source } => source.alert(),
            Self::NoCommonParameters { source } => source.alert(),
            Self::RetryKeyShare | Self::RetryOffer => AlertDescription::ILLEGAL_PARAMETER,
            Self::Binder { source } => source.alert(),
            Self::KeyExchange { source } => match source {
                ServerShareError::InvalidPeerShare { .. } => AlertDescription::ILLEGAL_PARAMETER,
                ServerShareError::KeyGeneration { .. } => AlertDescription::INTERNAL_ERROR,
            },
            Self::NoCommonScheme { source } => source.alert(),
            Self::Finished { source } => source.alert(),
            Self::ChoiceNotOffered
            | Self::TicketIndex { .. }
            | Self::Random { .. }
            | Self::EncryptedExtensions { .. }
            | Self::CertificateEntry { .. }
            | Self::Certificate { .. }
            | Self::Signing { .. }
            | Self::SignatureLength { .. }
            | Self::Ticket { .. }
            | Self::OutputStopped { .. } => AlertDescription::INTERNAL_ERROR,
        }
    }
}

impl<L, S, G, K> From<ReadFirstClientHelloError> for ServerAlertError<L, S, G, K> {
    fn from(error: ReadFirstClientHelloError) -> Self {
        match error {
            ReadFirstClientHelloError::Message { source } => Self::Message { source },
            ReadFirstClientHelloError::ClientHello { source } => Self::ClientHello { source },
        }
    }
}

/// The alert failures of [`ServerHandshake`] with the capabilities `L`,
/// `S`, `P` and `K`.
type AlertOf<L, S, P, K> = ServerAlertError<
    <L as HelloLayers>::Error,
    <S as TargetSelection>::Error,
    <<P as CertificateSelection<<S as TargetSelection>::Target>>::Key as SigningKey>::Error,
    <<<K as TicketPolicy<<S as TargetSelection>::Target>>::Issuing as Issuing>::Slot as TicketSlot>::Error,
>;

/// How a step fails with the alert failure `F` on the family `W` of the
/// target `T`.
type Failed<F, W, T> = HandshakeError<F, <W as ServerTransport<T>>::Error>;

fn alert<F, E>(source: F) -> HandshakeError<F, E> {
    HandshakeError::Alert { source }
}

/// Collects one handshake message from `host`.
async fn read_message<T, W, L, S, G, K>(
    host: &mut impl ReceiverAccess<Family = W>,
    limits: MessageLimits,
    hash: Option<HashAlgorithm>,
) -> Result<HandshakeMessage, Failed<ServerAlertError<L, S, G, K>, W, T>>
where
    W: ServerHandshakeCapability<T>,
{
    host.read_handshake_message(limits, hash)
        .await
        .map_err(|error| match split_read(error) {
            Ok(source) => alert(ServerAlertError::Message { source }),
            Err(error) => HandshakeError::Transport {
                source: W::convert_read_error(error),
            },
        })
}

/// Writes the bytes of one message to `host`.
async fn write_message<T, W, L, S, G, K>(
    host: &mut impl ReceiverAccess<Family = W>,
    message: &[u8],
) -> Result<(), Failed<ServerAlertError<L, S, G, K>, W, T>>
where
    W: ServerHandshakeCapability<T>,
{
    host.write_all(message)
        .await
        .map_err(|error| match split_write(error) {
            Ok(source) => alert(ServerAlertError::OutputStopped { source }),
            Err(error) => HandshakeError::Transport {
                source: W::convert_write_error(error),
            },
        })
}

/// Moves both streams of `host` to `level`.
fn move_to<T, W, F>(
    host: &mut impl LentAccess<Family = W>,
    level: CryptoLevel,
) -> Result<(), Failed<F, W, T>>
where
    W: ServerHandshakeCapability<T>,
{
    host.move_output_to(level);
    host.move_input_to(level)
        .map_err(|error| HandshakeError::Transport {
            source: W::convert_level_error(error),
        })
}

/// Reads the ClientHello of a whole message.
fn read_hello<L, S, G, K>(
    message: &HandshakeMessage,
) -> Result<ClientHello<Bytes>, ServerAlertError<L, S, G, K>> {
    read_client_hello(&mut message.encoded().clone())
        .map_err(|source| ServerAlertError::ClientHello { source })
}

/// Checks that a ClientHello does not ask for the middlebox compatibility
/// mode (RFC 9001 §8.4).
fn check_session_id<F, E>(hello: &ClientHello<Bytes>) -> Result<(), HandshakeError<F, E>> {
    LegacySessionIdError::check(*hello.legacy_session_id()).map_err(|source| {
        HandshakeError::ProtocolViolation {
            source: source.into(),
        }
    })
}

/// The `quic_transport_parameters` of a ClientHello.
fn transport_parameters(hello: &ClientHello<Bytes>) -> Option<&Bytes> {
    hello
        .extensions()
        .iter()
        .find_map(|extension| match extension {
            ClientHelloExtension::QuicTransportParameters(data) => Some(data.data()),
            _ => None,
        })
}

/// The `psk_key_exchange_modes` of a ClientHello.
fn psk_modes(hello: &ClientHello<Bytes>) -> Option<&[PskKeyExchangeMode]> {
    hello
        .extensions()
        .iter()
        .find_map(|extension| match extension {
            ClientHelloExtension::PskKeyExchangeModes(modes) => Some(modes.as_slice()),
            _ => None,
        })
}

/// Checks what every ClientHello the server continues with must have:
/// TLS 1.3, (EC)DHE groups and shares, and transport parameters.
fn check_hello<L, S, G, K>(hello: &ClientHello<Bytes>) -> Result<(), ServerAlertError<L, S, G, K>> {
    let tls13 = hello.extensions().iter().any(|extension| {
        matches!(extension, ClientHelloExtension::SupportedVersions(versions)
            if versions.as_slice().contains(&ProtocolVersion::TLS_1_3))
    });
    if !tls13 {
        return Err(ServerAlertError::UnsupportedVersion);
    }
    let missing = |extension| ServerAlertError::MissingExtension { extension };
    if hello.supported_groups().is_none() {
        return Err(missing(RequiredExtension::SupportedGroups));
    }
    if hello.key_share().is_none() {
        return Err(missing(RequiredExtension::KeyShare));
    }
    if hello.pre_shared_key().is_some() && psk_modes(hello).is_none() {
        return Err(missing(RequiredExtension::PskKeyExchangeModes));
    }
    if transport_parameters(hello).is_none() {
        return Err(missing(RequiredExtension::QuicTransportParameters));
    }
    Ok(())
}

/// A copy of the PSK of an opened ticket, which the chain consumes.
fn copy_psk(psk: &ResumptionPsk) -> ResumptionPsk {
    // invariant: the bytes come from a `Secret`.
    ResumptionPsk::new(Secret::new(psk.secret().as_bytes()).expect("a PSK fits a secret"))
}

/// The Early Secret the server continues with.
enum Early<C: TlsCrypto> {
    /// Without a PSK.
    Plain(EarlySecret<Server, C>),
    /// With the PSK of the identity at the index.
    Psk(PskEarlySecret<Server, C>, PskIndex),
}

/// Whether the client's age of its ticket is within `tolerance` of the
/// age the server measures (RFC 9846 §4.3.11.1, §8.3).
fn ticket_age_fits<T>(
    state: &TicketState<T>,
    obfuscated_age: u32,
    now: UtcDateTime,
    tolerance: Duration,
) -> bool {
    let stated =
        Duration::from_millis(u64::from(obfuscated_age.wrapping_sub(state.ticket_age_add)));
    let Some(measured) = elapsed(state.issued_at, now) else {
        return false;
    };
    stated.abs_diff(measured) <= tolerance
}

/// The start state of a server's handshake: the backend and the server's
/// capabilities, which [`ServerHandshakeSteps`] runs.
pub struct ServerHandshake<B, N, K, L, S, P, Q, Z, G> {
    backend: B,
    choice: N,
    tickets: K,
    layers: L,
    targets: S,
    identity: P,
    peer: Q,
    clock: Z,
    key_log: G,
    settings: ServerHandshakeSettings,
}

impl<B, N, K, L, S, P, Q, Z, G> ServerHandshake<B, N, K, L, S, P, Q, Z, G> {
    /// A server's handshake with `backend`, `choice`, `tickets`, `layers`,
    /// `targets`, `identity`, `peer`, `clock`, `key_log` and `settings`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        backend: B,
        choice: N,
        tickets: K,
        layers: L,
        targets: S,
        identity: P,
        peer: Q,
        clock: Z,
        key_log: G,
        settings: ServerHandshakeSettings,
    ) -> Self {
        Self {
            backend,
            choice,
            tickets,
            layers,
            targets,
            identity,
            peer,
            clock,
            key_log,
            settings,
        }
    }
}

/// The state of a server's handshake after its ServerHello is written: what
/// the rest of its flight needs.
///
/// `A` is the accepted ClientHello, `T` the type of the selected target and
/// `G` the key log.
pub struct ServerHelloWrittenState<B: TlsCrypto, K, P, Q, A, T, G> {
    backend: B,
    tickets: K,
    identity: P,
    peer: Q,
    key_log: G,
    limits: MessageLimits,
    accepted: A,
    selected: SelectedTarget<T>,
    suite: SupportedSuite<B>,
    negotiated: Negotiated,
    early_data: bool,
    resumed: bool,
    psk_dhe: bool,
    handshake: HandshakeSecret<Server, B>,
}

/// The state of a server's handshake after its Finished is written: what
/// verifying the client's Finished and issuing tickets need.
pub struct ServerFinishedWrittenState<B: TlsCrypto, K, T> {
    backend: B,
    tickets: K,
    limits: MessageLimits,
    selected: SelectedTarget<T>,
    suite: SupportedSuite<B>,
    local: OpaqueData<Bytes>,
    resumed: bool,
    psk_dhe: bool,
    main: MainSecret<Server, B>,
}

/// The state of a server's handshake once it is complete: what issuing
/// tickets needs.
pub struct ServerHandshakeCompletedState<B: TlsCrypto, K, T> {
    backend: B,
    tickets: K,
    /// What the tickets resume, `None` when the client cannot resume with
    /// them.
    session: Option<Resumable<T>>,
    resumption: ResumptionSecret<Server, B>,
}

impl<B, N, K, L, S, P, Q, Z, G> ServerHandshakeSteps for ServerHandshake<B, N, K, L, S, P, Q, Z, G>
where
    B: TlsCrypto + Clone,
    N: ParameterChoice<B, <S as TargetSelection>::Target>,
    K: TicketPolicy<<S as TargetSelection>::Target>,
    L: HelloLayers,
    L::Error: CallsForAlert,
    S: TargetSelection,
    S::Error: CallsForAlert,
    S::Target: Clone + PartialEq,
    P: CertificateSelection<<S as TargetSelection>::Target>,
    Q: PeerAuthentication<<S as TargetSelection>::Target, Request = Infallible>,
    Z: Clock,
    G: KeyLog,
{
    type Crypto = B;
    type Target = S::Target;
    type AlertError = AlertOf<L, S, P, K>;
    type ServerHelloWrittenState = ServerHelloWrittenState<B, K, P, Q, L::Accepted, S::Target, G>;
    type ServerFinishedWrittenState = ServerFinishedWrittenState<B, K, S::Target>;
    type ServerHandshakeCompletedState = ServerHandshakeCompletedState<B, K, S::Target>;

    async fn write_server_hello<W: ServerHandshakeCapability<S::Target>>(
        self,
        first: FirstClientHello,
        host: &mut impl LentAccess<Family = W>,
    ) -> Result<(ServerHelloWritten<S::Target>, Self::ServerHelloWrittenState), StepError<Self, W>>
    {
        let Self {
            backend,
            choice,
            tickets,
            layers,
            targets,
            identity,
            peer,
            clock,
            key_log,
            settings,
        } = self;
        let limits = settings.limits;

        // The layers of the first ClientHello, the target and the parameters.
        let FirstClientHello {
            encoded,
            client_hello,
        } = first;
        let accepted = layers
            .open(encoded, client_hello, None)
            .await
            .map_err(|source| alert(ServerAlertError::Layers { source }))?;
        check_hello(accepted.client_hello()).map_err(alert)?;
        let selected = targets
            .select(accepted.client_hello())
            .await
            .map_err(|source| alert(ServerAlertError::Target { source }))?;
        let (chosen, needs_retry) = {
            let offer = ClientOffer::from_client_hello(accepted.client_hello());
            let chosen = choice
                .choose(&backend, &offer, &selected)
                .map_err(|source| alert(ServerAlertError::NoCommonParameters { source }))?;
            if !offer.cipher_suites().contains(&chosen.suite().suite())
                || !offer
                    .supported_groups()
                    .contains(&chosen.group().algorithm())
            {
                return Err(alert(ServerAlertError::ChoiceNotOffered));
            }
            (chosen, !offer.has_key_share(chosen.group().algorithm()))
        };
        let suite: SupportedSuite<B> = chosen.suite();
        let group = chosen.group();

        // A HelloRetryRequest when the client has no share of the group.
        let (accepted, transcript, retried) = if needs_retry {
            let retry = HelloRetryRequest::new(
                ProtocolVersion::TLS_1_2,
                *accepted.client_hello().legacy_session_id(),
                suite.suite(),
                vec![
                    HelloRetryRequestExtension::<Bytes>::SupportedVersions(
                        ProtocolVersion::TLS_1_3,
                    ),
                    HelloRetryRequestExtension::KeyShare(group.algorithm()),
                ],
            )
            // invariant: two short extensions fit the block.
            .expect("a HelloRetryRequest of two extensions");
            let encoded = encode(retry.encoded_len(), |sink| {
                write_hello_retry_request(sink, &retry)
            });
            write_message::<S::Target, W, _, _, _, _>(host, &encoded).await?;
            let transcript = Transcript::<Server, B>::new(&backend, suite.hash())
                .add(accepted.encoded().encoded())
                .retry(&encoded);
            let second = read_message::<S::Target, W, _, _, _, _>(host, limits, None).await?;
            let hello = read_hello(&second).map_err(alert)?;
            check_session_id(&hello)?;
            let accepted = layers
                .open(second, hello, Some(&accepted))
                .await
                .map_err(|source| alert(ServerAlertError::Layers { source }))?;
            let hello = accepted.client_hello();
            check_hello(hello).map_err(alert)?;
            let offer = ClientOffer::from_client_hello(hello);
            if !offer.cipher_suites().contains(&suite.suite())
                || !offer.supported_groups().contains(&group.algorithm())
            {
                return Err(alert(ServerAlertError::RetryOffer));
            }
            if offer.key_share_groups() != [group.algorithm()] {
                return Err(alert(ServerAlertError::RetryKeyShare));
            }
            (accepted, transcript, true)
        } else {
            let transcript = Transcript::<Server, B>::new(&backend, suite.hash());
            (accepted, transcript, false)
        };
        let hello = accepted.client_hello();
        let encoded = accepted.encoded().encoded().clone();
        let client_share = hello
            .key_share()
            .and_then(|shares| shares.get(group.algorithm()))
            // invariant: without a share of the group, the server asked for one,
            // and the second ClientHello was checked to carry it.
            .expect("a share of the chosen group");
        let peer_parameters = transport_parameters(hello)
            // invariant: `check_hello` requires the extension.
            .expect("host parameters")
            .clone();

        // Whether the client resumes with psk_dhe_ke, the one mode the server
        // takes: it restricts both the offered tickets and the issued ones
        // (RFC 9846 §4.3.9).
        let psk_dhe =
            psk_modes(hello).is_some_and(|modes| modes.contains(&PskKeyExchangeMode::PSK_DHE_KE));

        // An offered ticket, with its binder.
        let offered: Option<&OfferedPsks<Bytes>> = hello.pre_shared_key().filter(|_| psk_dhe);
        let opened = match offered {
            Some(offered) => match tickets.open(offered).await {
                Some((index, opened)) => {
                    if usize::from(index.get()) >= offered.identities().len() {
                        return Err(alert(ServerAlertError::TicketIndex { index }));
                    }
                    let state = opened.state();
                    let fits = state.resumable.suite.hash() == Some(suite.hash().algorithm())
                        && state.resumable.target == selected.target;
                    fits.then_some((index, opened))
                }
                None => None,
            },
            None => None,
        };
        let early = match &opened {
            Some((index, opened)) => {
                // invariant: a ClientHello with `pre_shared_key` has binders.
                let covered = encoded.len() - hello.binders_len().expect("binders");
                let early = transcript
                    .add(&encoded[..covered])
                    .early_secret_with_psk(copy_psk(opened.psk()));
                // invariant: the index was checked against the identities, and
                // there is one binder for each.
                let binder = &offered.expect("an offer").binders()[usize::from(index.get())];
                early
                    .verify_binder(binder.binder().as_ref())
                    .map_err(|source| alert(ServerAlertError::Binder { source }))?;
                Early::Psk(early.add(&encoded[covered..]), *index)
            }
            None => Early::Plain(transcript.add(&encoded).early_secret()),
        };

        // The host parameters, and 0-RTT by all three.
        let negotiated = host
            .negotiate(
                &peer_parameters,
                &selected,
                opened.as_ref().map(|(_, opened)| opened.state()),
            )
            .map_err(|error| HandshakeError::Transport { source: error })?;
        let zero_rtt = match (opened, &early) {
            (Some((index, opened)), Early::Psk(early, _)) => {
                let state = opened.state();
                let offered_age = offered.expect("an offer").identities()[usize::from(index.get())]
                    .obfuscated_ticket_age();
                let handshake_allows = !retried
                    && index == PskIndex::new(0)
                    && hello
                        .extensions()
                        .iter()
                        .any(|extension| matches!(extension, ClientHelloExtension::EarlyData))
                    && state.early_data
                    && state.resumable.suite == suite.suite()
                    && state.resumable.application_protocol == selected.application_protocol
                    && ticket_age_fits(
                        state,
                        offered_age,
                        clock.now(),
                        settings.ticket_age_tolerance,
                    );
                if handshake_allows && negotiated.early_data {
                    let mut digest = Hash::start(&backend, suite.hash());
                    digest.update(&encoded);
                    let key = ClientHelloKey::new(digest.finish());
                    opened
                        .admit_early_data(&key)
                        .await
                        .then(|| early.early_receive())
                } else {
                    None
                }
            }
            _ => None,
        };
        let early_data = zero_rtt.is_some();

        // The ServerHello and the Handshake secrets.
        let (server_share, shared) =
            KeyExchange::server_share(&backend, group, client_share.key_exchange().as_ref())
                .map_err(|source| alert(ServerAlertError::KeyExchange { source }))?;
        let random: [u8; 32] = Random::random(&backend)
            .map_err(|source| alert(ServerAlertError::Random { source }))?;
        let mut extensions = vec![
            ServerHelloExtension::SupportedVersions(ProtocolVersion::TLS_1_3),
            ServerHelloExtension::KeyShare(
                KeyShareEntry::new(
                    group.algorithm(),
                    Bytes::copy_from_slice(server_share.as_bytes()),
                )
                // invariant: a backend's share is the size of its group's share.
                .expect("a key share of the group"),
            ),
        ];
        let resumed = matches!(early, Early::Psk(..));
        if let Early::Psk(_, index) = &early {
            extensions.push(ServerHelloExtension::PreSharedKey(*index));
        }
        let server_hello = ServerHello::new(
            ProtocolVersion::TLS_1_2,
            HelloRandom(random),
            *hello.legacy_session_id(),
            suite.suite(),
            extensions,
        )
        // invariant: three short extensions fit the block, and a random equal to
        // that of a HelloRetryRequest is a broken backend.
        .expect("a ServerHello of three extensions");
        let encoded_hello = encode(server_hello.encoded_len(), |sink| {
            write_server_hello(sink, &server_hello)
        });
        write_message::<S::Target, W, _, _, _, _>(host, &encoded_hello).await?;
        let (handshake, secrets) = match early {
            Early::Plain(early) => early.server_hello(shared, &encoded_hello),
            Early::Psk(early, _) => early.server_hello(shared, &encoded_hello),
        };
        let client_random = accepted.client_hello().random();
        if let Some(early) = &zero_rtt {
            key_log.log(
                KeyLogLabel::ClientEarlyTrafficSecret,
                client_random,
                early.0.as_bytes(),
            );
        }
        key_log.log(
            KeyLogLabel::ClientHandshakeTrafficSecret,
            client_random,
            secrets.receive.as_bytes(),
        );
        key_log.log(
            KeyLogLabel::ServerHandshakeTrafficSecret,
            client_random,
            secrets.send.as_bytes(),
        );
        let written = ServerHelloWritten {
            aead: suite.aead(),
            secrets,
            selected: selected.clone(),
            zero_rtt,
        };
        let state = ServerHelloWrittenState {
            backend,
            tickets,
            identity,
            peer,
            key_log,
            limits,
            accepted,
            selected,
            suite,
            negotiated,
            early_data,
            resumed,
            psk_dhe,
            handshake,
        };
        Ok((written, state))
    }

    async fn write_server_finished<W: ServerHandshakeCapability<S::Target>>(
        state: Self::ServerHelloWrittenState,
        host: &mut impl LentAccess<Family = W>,
    ) -> Result<(OneRttSendTrafficSecret, Self::ServerFinishedWrittenState), StepError<Self, W>>
    {
        let ServerHelloWrittenState {
            backend,
            tickets,
            identity,
            peer,
            key_log,
            limits,
            accepted,
            selected,
            suite,
            negotiated,
            early_data,
            resumed,
            psk_dhe,
            handshake,
        } = state;
        move_to::<S::Target, W, _>(host, CryptoLevel::Handshake)?;
        let hello = accepted.client_hello();
        // EncryptedExtensions.
        let mut extensions = Vec::new();
        if let Some(protocol) = &selected.application_protocol {
            extensions.push(
                EncryptedExtensionsExtension::ApplicationLayerProtocolNegotiation(protocol.clone()),
            );
        }
        if early_data {
            extensions.push(EncryptedExtensionsExtension::EarlyData);
        }
        extensions.push(EncryptedExtensionsExtension::QuicTransportParameters(
            negotiated.local.clone(),
        ));
        let encrypted = EncryptedExtensions::new(extensions)
            .map_err(|source| alert(ServerAlertError::EncryptedExtensions { source }))?;
        let encoded = encode(encrypted.encoded_len(), |sink| {
            write_encrypted_extensions(sink, &encrypted)
        });
        write_message::<S::Target, W, _, _, _, _>(host, &encoded).await?;
        let mut handshake = handshake.add(&encoded);

        // Certificate and CertificateVerify, unless the PSK authenticates.
        if !resumed {
            if let Some(never) = peer.request(&selected) {
                match never {}
            }
            let offered =
                hello
                    .signature_algorithms()
                    .ok_or(alert(ServerAlertError::MissingExtension {
                        extension: RequiredExtension::SignatureAlgorithms,
                    }))?;
            let presented = identity
                .select(&selected, offered.as_slice())
                .map_err(|source| alert(ServerAlertError::NoCommonScheme { source }))?;
            let entries = presented
                .certificates()
                .iter()
                .map(|certificate| CertificateEntry::new(certificate.clone(), Vec::new()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|source| alert(ServerAlertError::CertificateEntry { source }))?;
            let certificate = Certificate::new(Bytes::new(), CertificateList::X509(entries))
                .map_err(|source| alert(ServerAlertError::Certificate { source }))?;
            let encoded = encode(certificate.encoded_len(), |sink| {
                write_certificate(sink, &certificate)
            });
            write_message::<S::Target, W, _, _, _, _>(host, &encoded).await?;
            handshake = handshake.add(&encoded);
            let content = handshake.certificate_verify_content();
            let signature = presented
                .sign(content.as_bytes())
                .await
                .map_err(|source| alert(ServerAlertError::Signing { source }))?;
            let verify = CertificateVerify::new(
                presented.scheme(),
                Bytes::copy_from_slice(signature.as_bytes()),
            )
            .map_err(|source| alert(ServerAlertError::SignatureLength { source }))?;
            let encoded = encode(verify.encoded_len(), |sink| {
                write_certificate_verify(sink, &verify)
            });
            write_message::<S::Target, W, _, _, _, _>(host, &encoded).await?;
            handshake = handshake.add(&encoded);
        }

        // The server's Finished, then the client's.
        let (main, finished, one_rtt_send) = handshake.server_finished();
        let client_random = hello.random();
        key_log.log(
            KeyLogLabel::ClientTrafficSecret0,
            client_random,
            main.client_traffic_secret().as_bytes(),
        );
        key_log.log(
            KeyLogLabel::ServerTrafficSecret0,
            client_random,
            one_rtt_send.0.as_bytes(),
        );
        key_log.log(
            KeyLogLabel::ExporterSecret,
            client_random,
            main.exporter_secret(),
        );
        write_message::<S::Target, W, _, _, _, _>(host, finished.encoded()).await?;
        let state = ServerFinishedWrittenState {
            backend,
            tickets,
            limits,
            selected,
            suite,
            local: negotiated.local,
            resumed,
            psk_dhe,
            main,
        };
        Ok((one_rtt_send, state))
    }

    async fn read_client_finished<W: ServerHandshakeCapability<S::Target>>(
        state: Self::ServerFinishedWrittenState,
        host: &mut impl LentAccess<Family = W>,
    ) -> Result<
        (
            ServerHandshakeCompleted<B, S::Target>,
            Self::ServerHandshakeCompletedState,
        ),
        StepError<Self, W>,
    > {
        let ServerFinishedWrittenState {
            backend,
            tickets,
            limits,
            selected,
            suite,
            local,
            resumed,
            psk_dhe,
            main,
        } = state;
        let received =
            read_message::<S::Target, W, _, _, _, _>(host, limits, Some(suite.hash().algorithm()))
                .await?;
        let (resumption, completed) = main
            .client_finished(&received)
            .map_err(|source| alert(ServerAlertError::Finished { source }))?;
        let session = psk_dhe.then(|| Resumable {
            suite: suite.suite(),
            application_protocol: selected.application_protocol.clone(),
            target: selected.target.clone(),
            transport_parameters: local.data().clone(),
        });
        let completed = ServerHandshakeCompleted {
            one_rtt_receive: completed.one_rtt_receive,
            exporter: completed.exporter,
            selected,
            resumed,
        };
        let state = ServerHandshakeCompletedState {
            backend,
            tickets,
            session,
            resumption,
        };
        Ok((completed, state))
    }

    async fn write_tickets<W: ServerHandshakeCapability<S::Target>>(
        state: Self::ServerHandshakeCompletedState,
        host: &mut impl LentAccess<Family = W>,
    ) -> Result<(), StepError<Self, W>> {
        let ServerHandshakeCompletedState {
            backend,
            tickets,
            session,
            resumption,
        } = state;
        move_to::<S::Target, W, _>(host, CryptoLevel::OneRtt)?;
        // The tickets, none when the client cannot resume with them.
        let Some(session) = session else {
            return Ok(());
        };
        let mut issuing = tickets.issuing(&session);
        let mut count: u64 = 0;
        while let Some(slot) = issuing.next().await {
            // invariant: eight bytes are within the 255 of a nonce.
            let nonce = TicketNonce::new(Bytes::copy_from_slice(&count.to_be_bytes()))
                .expect("an eight-byte nonce");
            count += 1;
            let ticket_age_add = u32::from_be_bytes(
                Random::random(&backend)
                    .map_err(|source| alert(ServerAlertError::Random { source }))?,
            );
            let ticket = slot
                .seal(resumption.ticket_psk(&nonce), &nonce, ticket_age_add)
                .await
                .map_err(|source| alert(ServerAlertError::Ticket { source }))?;
            let mut extensions = Vec::new();
            if ticket.early_data() {
                extensions.push(NewSessionTicketExtension::EarlyData(MaxEarlyDataSize::QUIC));
            }
            let message = NewSessionTicket::new(
                ticket.lifetime(),
                ticket_age_add,
                nonce,
                ticket.ticket().clone(),
                extensions,
            )
            // invariant: `Ticket` holds 1 to 65535 bytes, and one extension fits
            // the block.
            .expect("a NewSessionTicket of a ticket");
            let encoded = encode(message.encoded_len(), |sink| {
                write_new_session_ticket(sink, &message)
            });
            write_message::<S::Target, W, _, _, _, _>(host, &encoded).await?;
        }
        Ok(())
    }
}

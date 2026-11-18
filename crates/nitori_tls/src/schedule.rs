//! The transcript and the key schedule of TLS 1.3 (RFC 9846 §4.1, §7.1),
//! as one chain of values that move.
//!
//! Each value holds the backend, the hash of the cipher suite and the
//! running transcript hash, and its methods are what the protocol can do at
//! that point; a step consumes the value before it. Without a ServerHello
//! there is no Handshake traffic secret, and without the server's Finished
//! there is no 1-RTT traffic secret.
//!
//! ```text
//! Transcript ── early_secret ──────────▶ EarlySecret ────┐
//!            └─ early_secret_with_psk ─▶ PskEarlySecret ─┴─ server_hello ─▶ HandshakeSecret
//!
//! server: HandshakeSecret<Server> ── server_finished ─▶ MainSecret<Server>
//!             ── client_finished(received)? ─▶ ResumptionSecret<Server> + Completed
//! client: HandshakeSecret<Client> ── authentication ─┬▶ AwaitingCertificateAuthentication ─┐
//!                                                    │      verify_certificate_verify?     │
//!                                                    └▶ Authenticated ◀────────────────────┘
//!             Authenticated ── server_finished(received)? ─▶ MainSecret<Client>
//!             ── client_finished ─▶ ResumptionSecret<Client>
//! ```
//!
//! The bytes of a message go into the transcript as they are, the same
//! bytes that are sent or were received: the caller adds them with `add`,
//! and the steps that produce a Finished message hand out its encoding.
//! Forks, such as a PSK binder, hash a clone of the running hash and leave
//! the chain as it is. Secrets are dropped as their steps consume them, and
//! a dropped secret is overwritten.
//!
//! The chain takes the backend's answer that it supports the hash, so no
//! step fails for lack of support. Traffic secrets leave the chain as
//! [`TrafficSecret`] values, each with the hash of the suite, for the
//! protection of QUIC packets to derive its keys from.

// `add` of each step adds a message to the transcript and returns the same
// step by value; it is not addition.
#![allow(clippy::should_implement_trait)]

use crate::{
    crypto::TlsCrypto,
    extension::pre_shared_key::PskIndex,
    message::{
        HandshakeMessage,
        certificate_verify::codec::read_certificate_verify,
        finished::{
            Finished,
            codec::{ReadFinishedError, read_finished, write_finished},
        },
        message_hash::{MessageHash, codec::write_message_hash},
        new_session_ticket::TicketNonce,
    },
    role::{Client, HandshakeRole, Server},
    vector::LengthOutOfRangeError,
    width::Width,
};
use bytes::Bytes;
use core::marker::PhantomData;
use nitori_crypto::{
    Digest, ExpandLength, Hash, HashAlgorithm, Hmac, RunningHash, Secret, SignatureVerify,
    Supported, TagMismatchError,
};
use snafu::{ResultExt, Snafu, ensure};

mod client;

pub use client::{
    Authenticated, Authentication, AwaitingCertificateAuthentication, CertificateVerifyError,
};

/// The longest label an HkdfLabel holds after its "tls13 " prefix.
const LABEL_MAX: usize = Width::U8.max() - 6;

/// What every value of the chain holds: the backend, the hash and the
/// running transcript hash.
struct Chain<C: TlsCrypto> {
    backend: C,
    hash: Supported<C, HashAlgorithm>,
    transcript: C::Running,
}

impl<C: TlsCrypto + Clone> Clone for Chain<C> {
    fn clone(&self) -> Self {
        Self {
            backend: self.backend.clone(),
            hash: self.hash,
            transcript: self.transcript.clone(),
        }
    }
}

impl<C: TlsCrypto> Chain<C> {
    fn algorithm(&self) -> HashAlgorithm {
        self.hash.algorithm()
    }

    fn hash_len(&self) -> ExpandLength {
        ExpandLength::hash_len(self.algorithm())
    }

    fn add(&mut self, message: &[u8]) {
        self.transcript.update(message);
    }

    /// The hash of the transcript so far, from a fork of the running hash.
    fn current(&self) -> Digest {
        self.transcript.clone().finish()
    }

    fn empty_hash(&self) -> Digest {
        Hash::start(&self.backend, self.hash).finish()
    }

    fn extract(&self, salt: &[u8], ikm: &[u8]) -> Secret {
        self.backend.extract(self.hash, salt, ikm)
    }

    /// A string of zeros as long as the hash, the salt and input of the
    /// extractions without a PSK or a shared secret.
    fn zeros(&self) -> Secret {
        Secret::zeroed(self.hash_len())
    }

    fn expand_label(
        &self,
        secret: &Secret,
        label: &[u8],
        context: &[u8],
        len: ExpandLength,
    ) -> Secret {
        expand_label(&self.backend, self.hash, secret, label, context, len)
    }

    fn derive_secret(&self, secret: &Secret, label: &[u8], hash: &Digest) -> Secret {
        self.expand_label(secret, label, hash.as_bytes(), self.hash_len())
    }

    /// HMAC under the Finished key of `base` over the transcript so far: a
    /// Finished message and a PSK binder are both this
    /// (RFC 9846 §4.5.3, §4.3.11.2).
    fn verify_data(&self, base: &Secret) -> Digest {
        let key = self.expand_label(base, b"finished", &[], self.hash_len());
        Hmac::sign(
            &self.backend,
            self.hash,
            key.as_bytes(),
            self.current().as_bytes(),
        )
    }

    /// Checks `received` against [`Chain::verify_data`] in constant time.
    fn check_verify_data(&self, base: &Secret, received: &[u8]) -> Result<(), TagMismatchError> {
        let key = self.expand_label(base, b"finished", &[], self.hash_len());
        Hmac::verify(
            &self.backend,
            self.hash,
            key.as_bytes(),
            self.current().as_bytes(),
            received,
        )
    }

    fn traffic(&self, secret: &Secret) -> TrafficSecret {
        traffic(self.algorithm(), secret)
    }

    /// The encoding of the Finished message for `base`, added to the
    /// transcript.
    fn write_finished(&mut self, base: &Secret) -> HandshakeMessage {
        let finished = Finished::new(self.verify_data(base));
        let mut encoded = Vec::with_capacity(finished.encoded_len());
        write_finished(&mut encoded, &finished);
        self.add(&encoded);
        HandshakeMessage::from_encoded(Bytes::from(encoded))
    }

    /// Reads a received Finished message and checks it against the
    /// Finished key of `base`, then adds it to the transcript.
    fn read_finished(
        &mut self,
        base: &Secret,
        received: &HandshakeMessage,
    ) -> Result<(), FinishedError> {
        let encoded = received.encoded().as_ref();
        let finished =
            read_finished(&mut { encoded }, self.algorithm()).context(finished_error::ReadSnafu)?;
        self.check_verify_data(base, finished.verify_data().as_bytes())
            .map_err(|TagMismatchError| FinishedError::Mismatch)?;
        self.add(encoded);
        Ok(())
    }
}

/// HKDF-Expand-Label (RFC 9846 §7.1). The "tls13 " prefix belongs to TLS,
/// not to the backend. The caller keeps `label` within [`LABEL_MAX`] bytes
/// and `context` within 255.
fn expand_label<C: TlsCrypto>(
    backend: &C,
    hash: Supported<C, HashAlgorithm>,
    secret: &Secret,
    label: &[u8],
    context: &[u8],
    len: ExpandLength,
) -> Secret {
    debug_assert!(label.len() <= LABEL_MAX && context.len() <= Width::U8.max());
    let length = u16::from(len.get()).to_be_bytes();
    let label_len = [(6 + label.len()) as u8];
    let context_len = [context.len() as u8];
    backend.expand(
        hash,
        secret,
        &[&length, &label_len, b"tls13 ", label, &context_len, context],
        len,
    )
}

/// A traffic secret of the suite's `hash`.
fn traffic(hash: HashAlgorithm, secret: &Secret) -> TrafficSecret {
    // invariant: every traffic secret is derived as long as the hash.
    TrafficSecret::new(hash, secret.as_bytes()).expect("a secret as long as the hash")
}

/// The start of the chain: the transcript before the Early Secret.
///
/// It holds no secret, so it can be cloned to fork the transcript, as a
/// client does to compute the binder of its first ClientHello while keeping
/// the transcript for a HelloRetryRequest.
pub struct Transcript<R, C: TlsCrypto> {
    chain: Chain<C>,
    role: PhantomData<R>,
}

impl<R, C: TlsCrypto + Clone> Clone for Transcript<R, C> {
    fn clone(&self) -> Self {
        Self {
            chain: self.chain.clone(),
            role: PhantomData,
        }
    }
}

impl<R: HandshakeRole, C: TlsCrypto + Clone> Transcript<R, C> {
    /// An empty transcript that hashes with `hash`, the hash of the selected
    /// cipher suite.
    pub fn new(backend: &C, hash: Supported<C, HashAlgorithm>) -> Self {
        Self {
            chain: Chain {
                backend: backend.clone(),
                hash,
                transcript: Hash::start(backend, hash),
            },
            role: PhantomData,
        }
    }

    /// Adds the bytes of a message, or of a part of one.
    pub fn add(mut self, message: &[u8]) -> Self {
        self.chain.add(message);
        self
    }

    /// Replaces the first ClientHello, which the transcript holds alone,
    /// with the message_hash message that stands for it, and adds the
    /// HelloRetryRequest (RFC 9846 §4.1).
    pub fn retry(mut self, hello_retry_request: &[u8]) -> Self {
        let message = MessageHash::new(self.chain.current());
        let mut buffer = [0; 4 + 48];
        // invariant: the buffer holds the longest hash, so it has room for
        // the message.
        write_message_hash(&mut &mut buffer[..], &message);
        let mut transcript = Hash::start(&self.chain.backend, self.chain.hash);
        transcript.update(&buffer[..message.encoded_len()]);
        transcript.update(hello_retry_request);
        self.chain.transcript = transcript;
        self
    }

    /// The Early Secret without a PSK: HKDF-Extract(0, 0) (RFC 9846 §7.1).
    pub fn early_secret(self) -> EarlySecret<R, C> {
        let zeros = self.chain.zeros();
        let early = self.chain.extract(zeros.as_bytes(), zeros.as_bytes());
        EarlySecret {
            chain: self.chain,
            early,
            role: PhantomData,
        }
    }

    /// The Early Secret of a resumption PSK, which this step consumes.
    ///
    /// The transcript holds the ClientHello up to its binders, which the
    /// binder covers; the rest of it is added to the value this returns.
    pub fn early_secret_with_psk(self, psk: ResumptionPsk) -> PskEarlySecret<R, C> {
        let zeros = self.chain.zeros();
        let early = self.chain.extract(zeros.as_bytes(), psk.0.as_bytes());
        PskEarlySecret {
            chain: self.chain,
            early,
            role: PhantomData,
        }
    }
}

/// The Early Secret without a PSK.
pub struct EarlySecret<R, C: TlsCrypto> {
    chain: Chain<C>,
    early: Secret,
    role: PhantomData<R>,
}

/// How the server authenticates in a handshake: with a certificate, or with
/// the PSK it accepted (RFC 9846 §4.2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Certificate,
    Psk,
}

/// Derives the Handshake Secret from the Early Secret `early` and the
/// shared secret, after adding the ServerHello.
fn handshake<R: HandshakeRole, C: TlsCrypto>(
    mut chain: Chain<C>,
    early: &Secret,
    shared: Secret,
    server_hello: &[u8],
    mode: Mode,
) -> (HandshakeSecret<R, C>, HandshakeTrafficSecrets) {
    let derived = chain.derive_secret(early, b"derived", &chain.empty_hash());
    let handshake = chain.extract(derived.as_bytes(), shared.as_bytes());
    drop(shared);
    chain.add(server_hello);
    let hash = chain.current();
    let client = chain.derive_secret(&handshake, b"c hs traffic", &hash);
    let server = chain.derive_secret(&handshake, b"s hs traffic", &hash);
    let (send, receive) = R::send_receive(chain.traffic(&client), chain.traffic(&server));
    (
        HandshakeSecret {
            chain,
            handshake,
            client,
            server,
            mode,
            role: PhantomData,
        },
        HandshakeTrafficSecrets { send, receive },
    )
}

impl<C: TlsCrypto + Clone> EarlySecret<Server, C> {
    /// Adds the ServerHello the server wrote and derives the Handshake
    /// Secret from the shared secret, which this step consumes.
    pub fn server_hello(
        self,
        shared: Secret,
        server_hello: &[u8],
    ) -> (HandshakeSecret<Server, C>, HandshakeTrafficSecrets) {
        handshake(
            self.chain,
            &self.early,
            shared,
            server_hello,
            Mode::Certificate,
        )
    }
}

impl<C: TlsCrypto + Clone> EarlySecret<Client, C> {
    /// Adds the ServerHello the client read and derives the Handshake Secret
    /// from the shared secret, which this step consumes.
    ///
    /// `selected` is the `pre_shared_key` of the ServerHello.
    ///
    /// # Errors
    ///
    /// [`UnofferedPskError`] if the ServerHello selects a PSK, since the client
    /// offered none.
    pub fn server_hello(
        self,
        shared: Secret,
        server_hello: &[u8],
        selected: Option<PskIndex>,
    ) -> Result<(HandshakeSecret<Client, C>, HandshakeTrafficSecrets), UnofferedPskError> {
        if let Some(index) = selected {
            return Err(UnofferedPskError { index });
        }
        Ok(handshake(
            self.chain,
            &self.early,
            shared,
            server_hello,
            Mode::Certificate,
        ))
    }
}

/// The Early Secret of a resumption PSK. Only this Early Secret has PSK
/// binders and 0-RTT.
pub struct PskEarlySecret<R, C: TlsCrypto> {
    chain: Chain<C>,
    early: Secret,
    role: PhantomData<R>,
}

impl<R: HandshakeRole, C: TlsCrypto + Clone> PskEarlySecret<R, C> {
    /// Adds the bytes of a message, or of a part of one: the binders and
    /// what follows them, once the binder is computed or checked.
    pub fn add(mut self, message: &[u8]) -> Self {
        self.chain.add(message);
        self
    }

    fn binder_key(&self) -> Secret {
        let chain = &self.chain;
        chain.derive_secret(&self.early, b"res binder", &chain.empty_hash())
    }

    /// client_early_traffic_secret, over the whole ClientHello.
    fn client_early_traffic(&self) -> TrafficSecret {
        let chain = &self.chain;
        let secret = chain.derive_secret(&self.early, b"c e traffic", &chain.current());
        chain.traffic(&secret)
    }
}

impl<C: TlsCrypto + Clone> PskEarlySecret<Client, C> {
    /// The binder of the PSK over the transcript so far, the ClientHello up
    /// to its binders (RFC 9846 §4.3.11.2).
    pub fn binder(&self) -> Digest {
        self.chain.verify_data(&self.binder_key())
    }

    /// The 0-RTT secret the client sends with, once the whole ClientHello is
    /// in the transcript.
    pub fn early_send(&self) -> ZeroRttSendTrafficSecret {
        ZeroRttSendTrafficSecret(self.client_early_traffic())
    }

    /// Adds the ServerHello the client read and derives the Handshake Secret
    /// from the shared secret, which this step consumes.
    ///
    /// `selected` is the `pre_shared_key` of the ServerHello: with the one
    /// PSK offered, the server authenticates with it; without, the server
    /// declined it and authenticates with a certificate, and the Handshake
    /// Secret comes from the Early Secret without a PSK.
    ///
    /// # Errors
    ///
    /// [`UnofferedPskError`] for an index other than 0, the one PSK offered.
    pub fn server_hello(
        self,
        shared: Secret,
        server_hello: &[u8],
        selected: Option<PskIndex>,
    ) -> Result<(HandshakeSecret<Client, C>, HandshakeTrafficSecrets), UnofferedPskError> {
        match selected {
            Some(index) if index != PskIndex::new(0) => Err(UnofferedPskError { index }),
            Some(_) => Ok(handshake(
                self.chain,
                &self.early,
                shared,
                server_hello,
                Mode::Psk,
            )),
            None => {
                drop(self.early);
                let zeros = self.chain.zeros();
                let early = self.chain.extract(zeros.as_bytes(), zeros.as_bytes());
                Ok(handshake(
                    self.chain,
                    &early,
                    shared,
                    server_hello,
                    Mode::Certificate,
                ))
            }
        }
    }
}

impl<C: TlsCrypto + Clone> PskEarlySecret<Server, C> {
    /// Checks the binder the client sent for the PSK over the transcript so
    /// far, the ClientHello up to its binders, in constant time
    /// (RFC 9846 §4.3.11).
    ///
    /// # Errors
    ///
    /// [`BinderMismatchError`] if it is not the binder.
    pub fn verify_binder(&self, binder: &[u8]) -> Result<(), BinderMismatchError> {
        self.chain
            .check_verify_data(&self.binder_key(), binder)
            .map_err(|TagMismatchError| BinderMismatchError)
    }

    /// The 0-RTT secret the server receives with, once the whole ClientHello
    /// is in the transcript.
    pub fn early_receive(&self) -> ZeroRttReceiveTrafficSecret {
        ZeroRttReceiveTrafficSecret(self.client_early_traffic())
    }

    /// Adds the ServerHello the server wrote, which accepts the PSK, and
    /// derives the Handshake Secret from the shared secret, which this step
    /// consumes.
    pub fn server_hello(
        self,
        shared: Secret,
        server_hello: &[u8],
    ) -> (HandshakeSecret<Server, C>, HandshakeTrafficSecrets) {
        handshake(self.chain, &self.early, shared, server_hello, Mode::Psk)
    }
}

/// The Handshake Secret: from the ServerHello to the server's Finished.
pub struct HandshakeSecret<R, C: TlsCrypto> {
    chain: Chain<C>,
    handshake: Secret,
    client: Secret,
    server: Secret,
    mode: Mode,
    role: PhantomData<R>,
}

impl<R: HandshakeRole, C: TlsCrypto + Clone> HandshakeSecret<R, C> {
    /// Adds the bytes of a message: EncryptedExtensions, CertificateRequest,
    /// Certificate and CertificateVerify as they are sent or received.
    pub fn add(mut self, message: &[u8]) -> Self {
        self.chain.add(message);
        self
    }

    /// Whether the server authenticates with the PSK rather than a
    /// certificate.
    pub fn resumed(&self) -> bool {
        self.mode == Mode::Psk
    }

    /// Derives the Main Secret and the 1-RTT secrets, with the server's
    /// Finished in the transcript.
    fn main_secret(self) -> (MainParts<C>, TrafficSecret, TrafficSecret) {
        let chain = self.chain;
        let derived = chain.derive_secret(&self.handshake, b"derived", &chain.empty_hash());
        let zeros = chain.zeros();
        let main = chain.extract(derived.as_bytes(), zeros.as_bytes());
        let hash = chain.current();
        let client = chain.derive_secret(&main, b"c ap traffic", &hash);
        let server = chain.derive_secret(&main, b"s ap traffic", &hash);
        let exporter = chain.derive_secret(&main, b"exp master", &hash);
        let (client, server) = (chain.traffic(&client), chain.traffic(&server));
        (
            MainParts {
                chain,
                main,
                client_handshake: self.client,
                exporter,
            },
            client,
            server,
        )
    }
}

impl<C: TlsCrypto + Clone> HandshakeSecret<Server, C> {
    /// The content the server signs for its CertificateVerify, over the
    /// transcript up to its Certificate (RFC 9846 §4.5.2).
    pub fn certificate_verify_content(&self) -> CertificateVerifyContent {
        CertificateVerifyContent::server(&self.chain.current())
    }

    /// Writes the server's Finished, which comes before the client's, and
    /// derives the Main Secret: the encoding of the Finished message, in
    /// the transcript now, and the server's 1-RTT secret to send with.
    pub fn server_finished(
        mut self,
    ) -> (
        MainSecret<Server, C>,
        HandshakeMessage,
        OneRttSendTrafficSecret,
    ) {
        let finished = self.chain.write_finished(&self.server);
        let (parts, client, server) = self.main_secret();
        (
            MainSecret {
                parts,
                withheld: client,
            },
            finished,
            OneRttSendTrafficSecret(server),
        )
    }
}

/// What the Main Secret step holds for both roles.
struct MainParts<C: TlsCrypto> {
    chain: Chain<C>,
    main: Secret,
    client_handshake: Secret,
    exporter: Secret,
}

/// The Main Secret: from the server's Finished to the client's.
pub struct MainSecret<R: HandshakeRole, C: TlsCrypto> {
    parts: MainParts<C>,
    withheld: R::Withheld,
}

impl<R: HandshakeRole, C: TlsCrypto + Clone> MainSecret<R, C> {
    /// Adds the bytes of a message: the client's Certificate and
    /// CertificateVerify, as they are sent or received.
    pub fn add(mut self, message: &[u8]) -> Self {
        self.parts.chain.add(message);
        self
    }

    /// exporter_secret, for a key log.
    pub(crate) fn exporter_secret(&self) -> &[u8] {
        self.parts.exporter.as_bytes()
    }

    /// The exporter of the connection, with a copy of
    /// exporter_secret.
    pub fn exporter(&self) -> Exporter<C> {
        let chain = &self.parts.chain;
        Exporter {
            backend: chain.backend.clone(),
            hash: chain.hash,
            // invariant: the secret is as long as the hash, which a secret
            // holds.
            secret: Secret::new(self.parts.exporter.as_bytes()).expect("a hash-long secret"),
        }
    }
}

impl<C: TlsCrypto> MainParts<C> {
    /// Derives resumption_secret, with the client's Finished in the
    /// transcript.
    fn resumption<R>(self) -> ResumptionSecret<R, C> {
        let chain = self.chain;
        let resumption = chain.derive_secret(&self.main, b"res master", &chain.current());
        ResumptionSecret {
            backend: chain.backend,
            hash: chain.hash,
            resumption,
            role: PhantomData,
        }
    }
}

impl<C: TlsCrypto + Clone> MainSecret<Server, C> {
    /// The client's 1-RTT secret, which the server receives with only once
    /// the client's Finished is verified, for a key log.
    pub(crate) fn client_traffic_secret(&self) -> &TrafficSecret {
        &self.withheld
    }

    /// Checks the client's Finished and adds it: the handshake is complete,
    /// and the client's 1-RTT secret is handed over with the exporter.
    ///
    /// # Errors
    ///
    /// [`FinishedError`] if `received` is not a Finished message of the
    /// suite's hash or not the client's Finished.
    pub fn client_finished(
        mut self,
        received: &HandshakeMessage,
    ) -> Result<(ResumptionSecret<Server, C>, Completed<C>), FinishedError> {
        let parts = &mut self.parts;
        parts
            .chain
            .read_finished(&parts.client_handshake, received)?;
        let exporter = self.exporter();
        let Self { parts, withheld } = self;
        Ok((
            parts.resumption(),
            Completed {
                one_rtt_receive: OneRttReceiveTrafficSecret(withheld),
                exporter,
            },
        ))
    }
}

impl<C: TlsCrypto + Clone> MainSecret<Client, C> {
    /// Writes the client's Finished, which comes after the server's: the
    /// encoding of the Finished message, in the transcript now.
    pub fn client_finished(mut self) -> (ResumptionSecret<Client, C>, HandshakeMessage) {
        let parts = &mut self.parts;
        let finished = parts.chain.write_finished(&parts.client_handshake);
        (self.parts.resumption(), finished)
    }
}

/// After the client's Finished: resumption_secret, from which the
/// PSK of each ticket is derived.
pub struct ResumptionSecret<R, C: TlsCrypto> {
    backend: C,
    hash: Supported<C, HashAlgorithm>,
    resumption: Secret,
    role: PhantomData<R>,
}

impl<R: HandshakeRole, C: TlsCrypto> ResumptionSecret<R, C> {
    /// The PSK of the ticket with `nonce`:
    /// HKDF-Expand-Label(resumption_secret, "resumption", nonce,
    /// Hash.length) (RFC 9846 §4.7.1).
    pub fn ticket_psk<N: AsRef<[u8]>>(&self, nonce: &TicketNonce<N>) -> ResumptionPsk {
        let len = ExpandLength::hash_len(self.hash.algorithm());
        ResumptionPsk(expand_label(
            &self.backend,
            self.hash,
            &self.resumption,
            b"resumption",
            nonce.nonce().as_ref(),
            len,
        ))
    }
}

/// A resumption PSK: the secret of a ticket.
pub struct ResumptionPsk(Secret);

impl ResumptionPsk {
    /// The PSK `secret`, such as one a ticket store kept.
    pub fn new(secret: Secret) -> Self {
        Self(secret)
    }

    /// The secret, for storing with its ticket.
    pub fn secret(&self) -> &Secret {
        &self.0
    }
}

/// The label of an exported value: 1 to 249 bytes, so that with the
/// "tls13 " prefix it fits the 7 to 255 bytes of an HkdfLabel's label
/// (RFC 9846 §7.1, §7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExporterLabel<L> {
    label: L,
}

impl<L: AsRef<[u8]>> ExporterLabel<L> {
    /// The label `label`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] for an empty label or one above 249 bytes.
    pub fn new(label: L) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(label.as_ref().len(), 1..=LABEL_MAX)?;
        Ok(Self { label })
    }
}

impl<L> ExporterLabel<L> {
    /// The label.
    pub fn label(&self) -> &L {
        &self.label
    }
}

/// The exporter of a connection (RFC 9846 §7.5): exporter_secret
/// with the hash and the backend.
pub struct Exporter<C: TlsCrypto> {
    backend: C,
    hash: Supported<C, HashAlgorithm>,
    secret: Secret,
}

impl<C: TlsCrypto> Exporter<C> {
    /// TLS-Exporter(label, context, len): HKDF-Expand-Label of
    /// Derive-Secret(exporter_secret, label, "") with "exporter" and
    /// the hash of `context`.
    pub fn export<L: AsRef<[u8]>>(
        &self,
        label: &ExporterLabel<L>,
        context: &[u8],
        len: ExpandLength,
    ) -> Secret {
        let label = label.label.as_ref();
        let hash_len = ExpandLength::hash_len(self.hash.algorithm());
        let empty = Hash::start(&self.backend, self.hash).finish();
        let derived = expand_label(
            &self.backend,
            self.hash,
            &self.secret,
            label,
            empty.as_bytes(),
            hash_len,
        );
        let mut context_hash = Hash::start(&self.backend, self.hash);
        context_hash.update(context);
        expand_label(
            &self.backend,
            self.hash,
            &derived,
            b"exporter",
            context_hash.finish().as_bytes(),
            len,
        )
    }
}

/// A TLS 1.3 traffic secret for one encryption level and one direction
/// (RFC 9846 §7.1), together with the hash of the cipher suite that
/// produced it.
///
/// The key schedule hands traffic secrets over; the keys that protect
/// packets or records are derived from them with HKDF over
/// [`hash`](Self::hash). A traffic secret derived outside the key schedule,
/// such as the Initial secret of QUIC (RFC 9001 §5.2), is built with
/// [`new`](Self::new), or with [`zeroed`](Self::zeroed) and filled in
/// place.
///
/// The secret bytes are overwritten when the value is dropped. `Debug` does
/// not print them.
pub struct TrafficSecret {
    hash: HashAlgorithm,
    secret: Secret,
}

impl TrafficSecret {
    /// Takes a copy of `secret`, which was derived with `hash`.
    ///
    /// # Errors
    ///
    /// Fails when `secret` is not exactly `hash.output_len()` bytes long.
    pub fn new(hash: HashAlgorithm, secret: &[u8]) -> Result<Self, SecretLengthMismatchError> {
        let expected = hash.output_len();
        ensure!(
            secret.len() == expected,
            SecretLengthMismatchSnafu {
                expected,
                actual: secret.len()
            }
        );
        let mut traffic = Self::zeroed(hash);
        traffic.as_mut_bytes().copy_from_slice(secret);
        Ok(traffic)
    }

    /// An all-zero secret for `hash`, to be filled through
    /// [`as_mut_bytes`](Self::as_mut_bytes).
    #[must_use]
    pub fn zeroed(hash: HashAlgorithm) -> Self {
        Self {
            hash,
            secret: Secret::zeroed(ExpandLength::hash_len(hash)),
        }
    }

    /// The hash this secret was derived with, which every derivation from
    /// it uses.
    #[must_use]
    pub fn hash(&self) -> HashAlgorithm {
        self.hash
    }

    /// The secret bytes, `self.hash().output_len()` of them.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.secret.as_bytes()
    }

    /// The secret bytes, for writing in place; the length does not change.
    #[must_use]
    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        self.secret.as_mut_bytes()
    }
}

impl core::fmt::Debug for TrafficSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TrafficSecret")
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

/// A secret's length does not match the output length of its hash.
#[derive(Debug, Snafu)]
#[snafu(display("expected a {expected}-byte secret, got {actual} bytes"))]
pub struct SecretLengthMismatchError {
    /// The output length of the hash.
    pub expected: usize,
    /// The length of the given secret.
    pub actual: usize,
}

/// The Handshake traffic secrets, as the role sends and receives.
pub struct HandshakeTrafficSecrets {
    /// The secret this endpoint sends with.
    pub send: TrafficSecret,
    /// The secret this endpoint receives with.
    pub receive: TrafficSecret,
}

/// The 1-RTT secret a server sends with once it has written its Finished.
pub struct OneRttSendTrafficSecret(pub TrafficSecret);

/// The 1-RTT secrets of a client that has verified the server's Finished.
pub struct OneRttTrafficSecrets {
    /// The secret the client sends with.
    pub send: TrafficSecret,
    /// The secret the client receives with.
    pub receive: TrafficSecret,
}

/// The client's 1-RTT secret, as a server receives with it once it has
/// verified the client's Finished.
pub struct OneRttReceiveTrafficSecret(pub TrafficSecret);

/// client_early_traffic_secret, as a client sends 0-RTT with it.
pub struct ZeroRttSendTrafficSecret(pub TrafficSecret);

/// client_early_traffic_secret, as a server receives 0-RTT with it.
pub struct ZeroRttReceiveTrafficSecret(pub TrafficSecret);

/// What a server hands over when its handshake completes.
pub struct Completed<C: TlsCrypto> {
    /// The client's 1-RTT secret, the server's to receive with.
    pub one_rtt_receive: OneRttReceiveTrafficSecret,
    /// The exporter of the connection.
    pub exporter: Exporter<C>,
}

/// The content a server's CertificateVerify signs: 64 spaces, the context
/// string of a server, a zero byte and the transcript hash
/// (RFC 9846 §4.5.2).
#[derive(Clone, Copy)]
pub struct CertificateVerifyContent {
    bytes: [u8; 64 + 33 + 1 + 48],
    len: u8,
}

impl CertificateVerifyContent {
    fn server(hash: &Digest) -> Self {
        let context = b"TLS 1.3, server CertificateVerify";
        let mut bytes = [0x20; 64 + 33 + 1 + 48];
        bytes[64..97].copy_from_slice(context);
        bytes[97] = 0;
        let hash = hash.as_bytes();
        bytes[98..98 + hash.len()].copy_from_slice(hash);
        Self {
            bytes,
            len: (98 + hash.len()) as u8,
        }
    }

    /// The bytes to sign.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl core::fmt::Debug for CertificateVerifyContent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CertificateVerifyContent")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

/// Checks a CertificateVerify the server sent, over the transcript so far,
/// with `public_key`, and adds it.
fn verify_server_signature<C: TlsCrypto>(
    chain: &mut Chain<C>,
    certificate_verify: &HandshakeMessage,
    public_key: &[u8],
) -> Result<(), CertificateVerifyError> {
    use client::certificate_verify_error::*;
    let encoded = certificate_verify.encoded().as_ref();
    let message = read_certificate_verify(&mut { encoded }).context(ReadSnafu)?;
    let scheme = message.algorithm();
    let Some(supported) = chain.backend.signature_scheme(scheme) else {
        return UnsupportedSchemeSnafu { scheme }.fail();
    };
    let content = CertificateVerifyContent::server(&chain.current());
    SignatureVerify::verify(
        &chain.backend,
        supported,
        public_key,
        content.as_bytes(),
        message.signature(),
    )
    .context(SignatureSnafu)?;
    chain.add(encoded);
    Ok(())
}

/// A received Finished message is not the peer's Finished.
#[derive(Debug, Snafu)]
#[snafu(module(finished_error), visibility(pub(crate)))]
pub enum FinishedError {
    /// The message is not a Finished message of the suite's hash.
    #[snafu(display("Finished read failed"))]
    Read {
        /// The failure.
        source: ReadFinishedError,
    },
    /// The verify_data is not the HMAC of the transcript.
    #[snafu(display("Finished verify_data does not match the transcript"))]
    Mismatch,
}

impl FinishedError {
    /// The alert the failure calls for: decrypt_error for verify_data that
    /// does not match (RFC 9846 §4.5.3), and that of the read failure
    /// otherwise.
    pub fn alert(&self) -> crate::alert::AlertDescription {
        match self {
            Self::Read { source } => source.alert(),
            Self::Mismatch => crate::alert::AlertDescription::DECRYPT_ERROR,
        }
    }
}

/// A binder the client sent is not the binder of its PSK.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("PSK binder does not match the ClientHello"))]
pub struct BinderMismatchError;

impl BinderMismatchError {
    /// The alert the failure calls for: decrypt_error (RFC 9846 §6.2).
    pub fn alert(&self) -> crate::alert::AlertDescription {
        crate::alert::AlertDescription::DECRYPT_ERROR
    }
}

/// The ServerHello selects a PSK the client did not offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("server selected PSK {}, which was not offered", index.get()))]
pub struct UnofferedPskError {
    index: PskIndex,
}

impl UnofferedPskError {
    /// The index the server selected.
    pub const fn index(self) -> PskIndex {
        self.index
    }

    /// The alert the failure calls for: illegal_parameter
    /// (RFC 9846 §4.3.11).
    pub fn alert(&self) -> crate::alert::AlertDescription {
        crate::alert::AlertDescription::ILLEGAL_PARAMETER
    }
}

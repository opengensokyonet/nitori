//! The two ways a server authenticates to a client.
//!
//! After the ServerHello, a server authenticates with a certificate or with
//! the PSK it accepted, never both (RFC 9846 §4.2.1). The client's chain
//! splits there: only the certificate branch verifies a CertificateVerify,
//! and both branches meet at [`Authenticated`], the only value that reads
//! the server's Finished, so a client cannot skip authentication.

// `add` of each step adds a message to the transcript and returns the same
// step by value; it is not addition.
#![allow(clippy::should_implement_trait)]

use super::{
    FinishedError, HandshakeSecret, MainSecret, Mode, OneRttTrafficSecrets, verify_server_signature,
};
use crate::{
    alert::AlertDescription,
    crypto::TlsCrypto,
    message::{HandshakeMessage, certificate_verify::codec::ReadCertificateVerifyError},
    role::Client,
};
use nitori_crypto::{BadSignatureError, SignatureScheme};
use snafu::Snafu;

/// The branch of a client's handshake after EncryptedExtensions.
pub enum Authentication<C: TlsCrypto> {
    /// The server authenticates with a certificate.
    Certificate(AwaitingCertificateAuthentication<C>),
    /// The server accepted the PSK; it sends no Certificate or
    /// CertificateVerify (RFC 9846 §4.5.1), and the identity is the one the
    /// ticket recorded.
    Resumed(Authenticated<C>),
}

/// A client waiting for the server's certificate and its CertificateVerify.
pub struct AwaitingCertificateAuthentication<C: TlsCrypto> {
    handshake: HandshakeSecret<Client, C>,
}

/// A client whose server has authenticated, by certificate or by PSK.
pub struct Authenticated<C: TlsCrypto> {
    handshake: HandshakeSecret<Client, C>,
}

impl<C: TlsCrypto + Clone> HandshakeSecret<Client, C> {
    /// Splits by how the server authenticates, once EncryptedExtensions is
    /// in the transcript.
    pub fn authentication(self) -> Authentication<C> {
        match self.mode {
            Mode::Certificate => {
                Authentication::Certificate(AwaitingCertificateAuthentication { handshake: self })
            }
            Mode::Psk => Authentication::Resumed(Authenticated { handshake: self }),
        }
    }
}

impl<C: TlsCrypto + Clone> AwaitingCertificateAuthentication<C> {
    /// Adds the bytes of a message: CertificateRequest and Certificate, as
    /// they were received.
    pub fn add(mut self, message: &[u8]) -> Self {
        self.handshake.chain.add(message);
        self
    }

    /// Verifies the server's CertificateVerify over the transcript up to its
    /// Certificate with `public_key`, the key of the server's leaf
    /// certificate without its algorithm identifier, and adds it.
    ///
    /// # Errors
    ///
    /// [`CertificateVerifyError`].
    pub fn verify_certificate_verify(
        self,
        certificate_verify: &HandshakeMessage,
        public_key: &[u8],
    ) -> Result<Authenticated<C>, CertificateVerifyError> {
        let mut handshake = self.handshake;
        verify_server_signature(&mut handshake.chain, certificate_verify, public_key)?;
        Ok(Authenticated { handshake })
    }
}

impl<C: TlsCrypto + Clone> Authenticated<C> {
    /// Checks the server's Finished, which comes before the client's, and
    /// adds it: the client's 1-RTT secrets in both directions.
    ///
    /// # Errors
    ///
    /// [`FinishedError`] if `received` is not a Finished message of the
    /// suite's hash or not the server's Finished.
    pub fn server_finished(
        self,
        received: &HandshakeMessage,
    ) -> Result<(MainSecret<Client, C>, OneRttTrafficSecrets), FinishedError> {
        let mut handshake = self.handshake;
        handshake.chain.read_finished(&handshake.server, received)?;
        let (parts, send, receive) = handshake.main_secret();
        Ok((
            MainSecret {
                parts,
                withheld: (),
            },
            OneRttTrafficSecrets { send, receive },
        ))
    }
}

/// The server's CertificateVerify does not authenticate it.
#[derive(Debug, Snafu)]
#[snafu(module(certificate_verify_error), visibility(pub(crate)))]
pub enum CertificateVerifyError {
    /// The message is not a CertificateVerify.
    #[snafu(display("CertificateVerify read failed"))]
    Read {
        /// The failure.
        source: ReadCertificateVerifyError,
    },
    /// The backend does not verify the scheme, so the client did not offer
    /// it.
    #[snafu(display("CertificateVerify uses {scheme:?}, which was not offered"))]
    UnsupportedScheme {
        /// The scheme.
        scheme: SignatureScheme,
    },
    /// The signature does not verify.
    #[snafu(display("CertificateVerify signature rejected"))]
    Signature {
        /// The failure.
        source: BadSignatureError,
    },
}

impl CertificateVerifyError {
    /// The alert the failure calls for: decrypt_error for a signature that
    /// does not verify (RFC 9846 §4.5.2), illegal_parameter for a scheme the
    /// client did not offer, and that of the read failure otherwise.
    pub fn alert(&self) -> AlertDescription {
        match self {
            Self::Read { source } => source.alert(),
            Self::UnsupportedScheme { .. } => AlertDescription::ILLEGAL_PARAMETER,
            Self::Signature { .. } => AlertDescription::DECRYPT_ERROR,
        }
    }
}

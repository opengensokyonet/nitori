//! Logging the secrets of a handshake for debugging tools
//!
//! A [`KeyLog`] receives every TLS 1.3 secret the handshake engine derives,
//! with the client_random of the connection, so that a packet analyzer can
//! decrypt the connection's packets. The labels are those of the NSS
//! SSLKEYLOGFILE format, where each line is a label, the client_random and
//! the secret, both in hexadecimal. 1-RTT secrets after a key update are not
//! logged; a tool derives them from the `_0` secrets.
//!
//! Logging discloses all traffic of the connection. The engines log nothing
//! unless the host gives them a key log other than `()`, and this crate
//! reads no environment variables: writing a file and any locking are the
//! implementation's.

use crate::message::HelloRandom;

/// Receives the secrets of a handshake as the engine derives them.
pub trait KeyLog {
    /// Records `secret` under `label` for the connection whose ClientHello
    /// carried `client_random`.
    fn log(&self, label: KeyLogLabel, client_random: &HelloRandom, secret: &[u8]);
}

/// No key log: nothing is recorded.
impl KeyLog for () {
    fn log(&self, _label: KeyLogLabel, _client_random: &HelloRandom, _secret: &[u8]) {}
}

/// The secret a [`KeyLog`] receives, named as in the NSS SSLKEYLOGFILE
/// format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyLogLabel {
    /// client_early_traffic_secret, for 0-RTT.
    ClientEarlyTrafficSecret,
    /// client_handshake_traffic_secret.
    ClientHandshakeTrafficSecret,
    /// server_handshake_traffic_secret.
    ServerHandshakeTrafficSecret,
    /// client_application_traffic_secret_0.
    ClientTrafficSecret0,
    /// server_application_traffic_secret_0.
    ServerTrafficSecret0,
    /// exporter_secret.
    ExporterSecret,
}

impl KeyLogLabel {
    /// The label as an SSLKEYLOGFILE line starts with it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientEarlyTrafficSecret => "CLIENT_EARLY_TRAFFIC_SECRET",
            Self::ClientHandshakeTrafficSecret => "CLIENT_HANDSHAKE_TRAFFIC_SECRET",
            Self::ServerHandshakeTrafficSecret => "SERVER_HANDSHAKE_TRAFFIC_SECRET",
            Self::ClientTrafficSecret0 => "CLIENT_TRAFFIC_SECRET_0",
            Self::ServerTrafficSecret0 => "SERVER_TRAFFIC_SECRET_0",
            Self::ExporterSecret => "EXPORTER_SECRET",
        }
    }
}

impl<Log: KeyLog + ?Sized> KeyLog for &Log {
    fn log(&self, label: KeyLogLabel, client_random: &HelloRandom, secret: &[u8]) {
        (**self).log(label, client_random, secret);
    }
}

impl<Log: KeyLog + ?Sized> KeyLog for std::sync::Arc<Log> {
    fn log(&self, label: KeyLogLabel, client_random: &HelloRandom, secret: &[u8]) {
        (**self).log(label, client_random, secret);
    }
}

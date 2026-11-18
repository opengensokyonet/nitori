//! TLS 1.3 handshake messages, extensions, key schedule and handshake
//! engines for QUIC.
//!
//! The baseline is TLS 1.3 ([RFC 9846]) as QUIC version 1 uses it
//! ([RFC 9001]): handshake messages travel in CRYPTO frames, without the
//! TLS record layer, and the handshake hands traffic secrets to QUIC packet
//! protection.
//!
//! - [`message`]: handshake messages, collected whole from the byte stream
//!   of an encryption level, with a reader and a writer for each type.
//! - [`extension`]: the extensions of those messages.
//! - [`schedule`]: the transcript and the key schedule as one chain of
//!   values, for the [`role`] of a server and of a client, over a backend
//!   with the capabilities of [`crypto::TlsCrypto`].
//! - [`code_point`]: code point lists, such as cipher suites and groups.
//! - [`cipher_suite`], [`version`], [`alert`]: cipher suites, protocol
//!   versions and alert descriptions.
//! - [`vector`]: failures shared by the length-prefixed fields.
//! - [`clock`]: the current time, as a capability.
//! - [`key_log`]: the secrets of a handshake, for debugging tools, as a
//!   capability that logs nothing by default.
//! - [`verify`]: verification of a server's certificate chain, with
//!   rustls-webpki and Certificate Transparency.
//! - [`server`], [`client`]: the capabilities each role's handshake
//!   consults, such as the choice of parameters and what a ClientHello
//!   offers, with implementations for plain Web connections, and each
//!   role's handshake engine, [`server::engine`] and [`client::engine`].
//! - [`engine`]: what the two engines share: the CRYPTO streams they read
//!   and write at the current encryption level, and how a handshake fails.
//!
//! The contents of each message, once [`message::codec::read_handshake_message`]
//! has collected it whole from its CRYPTO stream, are read and written by
//! synchronous functions in the `codec` module of their type. The readers
//! run on the bytes of a whole message, a [`bytes::Buf`] that splits with
//! [`nitori_io::SplitTo`] such as `&[u8]` or `Bytes`, and their
//! variable-length fields are pieces of those bytes, without copying; the
//! writers write into a [`bytes::BufMut`]. `encoded_len` methods compute
//! lengths without writing. Each read failure answers the TLS alert it calls
//! for through `alert`.
//!
//! [RFC 9846]: https://www.rfc-editor.org/rfc/rfc9846.html
//! [RFC 9001]: https://www.rfc-editor.org/rfc/rfc9001.html
#![feature(impl_trait_in_assoc_type)]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod alert;
pub mod cipher_suite;
pub mod client;
pub mod clock;
pub mod code_point;
pub mod crypto;
pub mod engine;
pub mod extension;
pub mod key_log;
pub mod message;
pub mod role;
pub mod schedule;
pub mod server;
mod source;
#[cfg(test)]
mod test_support;
pub mod vector;
pub mod verify;
pub mod version;
mod width;

/// Programs that must not compile: the orders the chain of
/// [`schedule`] excludes.
///
/// A server writes its Finished before it reads the client's, so its
/// Handshake Secret has no `client_finished`:
///
/// ```compile_fail,E0599
/// use nitori_tls::{
///     crypto::TlsCrypto, message::HandshakeMessage, role::Server, schedule::HandshakeSecret,
/// };
///
/// fn verify_first<C: TlsCrypto + Clone>(
///     handshake: HandshakeSecret<Server, C>,
///     finished: &HandshakeMessage,
/// ) {
///     let _ = handshake.client_finished(finished);
/// }
/// ```
///
/// Without the Handshake Secret there are no 1-RTT secrets, so an Early
/// Secret has no `server_finished`:
///
/// ```compile_fail,E0599
/// use nitori_tls::{crypto::TlsCrypto, role::Server, schedule::EarlySecret};
///
/// fn skip_the_server_hello<C: TlsCrypto + Clone>(early: EarlySecret<Server, C>) {
///     let _ = early.server_finished();
/// }
/// ```
///
/// A server that accepted the PSK sends no CertificateVerify, so the
/// resumption branch of a client has no `verify_certificate_verify`:
///
/// ```compile_fail,E0599
/// use nitori_tls::{crypto::TlsCrypto, message::HandshakeMessage, schedule::Authenticated};
///
/// fn verify_resumed<C: TlsCrypto + Clone>(
///     resumed: Authenticated<C>,
///     certificate_verify: &HandshakeMessage,
/// ) {
///     let _ = resumed.verify_certificate_verify(certificate_verify, &[]);
/// }
/// ```
///
/// A client reads the server's Finished only once the server has
/// authenticated, so neither its Handshake Secret nor its certificate
/// branch has `server_finished`:
///
/// ```compile_fail,E0599
/// use nitori_tls::{
///     crypto::TlsCrypto, message::HandshakeMessage, role::Client, schedule::HandshakeSecret,
/// };
///
/// fn skip_authentication<C: TlsCrypto + Clone>(
///     handshake: HandshakeSecret<Client, C>,
///     finished: &HandshakeMessage,
/// ) {
///     let _ = handshake.server_finished(finished);
/// }
/// ```
///
/// ```compile_fail,E0599
/// use nitori_tls::{crypto::TlsCrypto, message::HandshakeMessage, schedule::AwaitingCertificateAuthentication};
///
/// fn skip_certificate_verify<C: TlsCrypto + Clone>(
///     certificate: AwaitingCertificateAuthentication<C>,
///     finished: &HandshakeMessage,
/// ) {
///     let _ = certificate.server_finished(finished);
/// }
/// ```
///
/// A client writes its Finished only after it has verified the server's:
///
/// ```compile_fail,E0599
/// use nitori_tls::{crypto::TlsCrypto, schedule::Authenticated};
///
/// fn write_first<C: TlsCrypto + Clone>(authenticated: Authenticated<C>) {
///     let _ = authenticated.client_finished();
/// }
/// ```
///
/// A step consumes the secret before it:
///
/// ```compile_fail,E0382
/// use nitori_crypto::Secret;
/// use nitori_tls::{crypto::TlsCrypto, role::Server, schedule::EarlySecret};
///
/// fn twice<C: TlsCrypto + Clone>(early: EarlySecret<Server, C>, a: Secret, b: Secret) {
///     let _ = early.server_hello(a, &[]);
///     let _ = early.server_hello(b, &[]);
/// }
/// ```
///
/// The same programs in the order of the protocol compile:
///
/// ```
/// use nitori_tls::{
///     crypto::TlsCrypto,
///     message::HandshakeMessage,
///     role::{Client, Server},
///     schedule::{Authentication, EarlySecret, HandshakeSecret},
/// };
///
/// fn server<C: TlsCrypto + Clone>(early: EarlySecret<Server, C>, shared: nitori_crypto::Secret, finished: &HandshakeMessage) {
///     let (handshake, _) = early.server_hello(shared, &[]);
///     let (master, _, _) = handshake.server_finished();
///     let _ = master.client_finished(finished);
/// }
///
/// fn client<C: TlsCrypto + Clone>(
///     handshake: HandshakeSecret<Client, C>,
///     certificate_verify: &HandshakeMessage,
///     finished: &HandshakeMessage,
/// ) {
///     let authenticated = match handshake.authentication() {
///         Authentication::Certificate(certificate) => {
///             let Ok(authenticated) = certificate.verify_certificate_verify(certificate_verify, &[])
///             else {
///                 return;
///             };
///             authenticated
///         }
///         Authentication::Resumed(authenticated) => authenticated,
///     };
///     if let Ok((master, _)) = authenticated.server_finished(finished) {
///         let _ = master.client_finished();
///     }
/// }
/// ```
#[cfg(doctest)]
pub struct CompileFail;

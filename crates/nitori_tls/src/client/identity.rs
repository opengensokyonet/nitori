//! What a client presents when the server asks for its certificate.
//!
//! A server that authenticates with a certificate may send a
//! CertificateRequest; the client then sends a Certificate, and with a
//! certificate in it a CertificateVerify, before its Finished. A client
//! without a certificate sends an empty Certificate (RFC 9846 §4.4.2).
//! [`ClientCertificateSelection::select`] decides for the target and the
//! request.
//!
//! [`NoClientCertificate`] presents none. What it presents is of a type
//! with no values, so the handshake always sends an empty Certificate.

use crate::{client::ServerTarget, message::certificate_request::CertificateRequest};
use bytes::Bytes;
use core::convert::Infallible;

/// How a client decides what it presents when the server asks for a
/// certificate.
pub trait ClientCertificateSelection {
    /// What the client presents in its Certificate.
    type Presented;

    /// What the client presents to `target` for `request`, or `None` to
    /// send an empty Certificate.
    fn select(
        &self,
        target: &ServerTarget,
        request: &CertificateRequest<Bytes>,
    ) -> Option<Self::Presented>;
}

/// Presents no certificate to any server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NoClientCertificate;

impl ClientCertificateSelection for NoClientCertificate {
    type Presented = Infallible;

    fn select(
        &self,
        _target: &ServerTarget,
        _request: &CertificateRequest<Bytes>,
    ) -> Option<Infallible> {
        None
    }
}

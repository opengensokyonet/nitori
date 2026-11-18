//! Whether a server asks the client for a certificate.
//!
//! A server that authenticates with a certificate may send a
//! CertificateRequest, and then reads the client's Certificate and
//! CertificateVerify before its Finished (RFC 9846 §4.4.2, §4.5.1).
//! [`PeerAuthentication::request`] decides for the selected target.
//!
//! [`NoCertificateRequest`] never asks. Its request is of a type with no
//! values, so the handshake's code for client certificates is never
//! reached.

use crate::server::targets::SelectedTarget;
use core::convert::Infallible;

/// How a server decides whether to ask for a client certificate.
///
/// `T` is the type of the selected target.
pub trait PeerAuthentication<T> {
    /// What a CertificateRequest asks for.
    type Request;

    /// The request for `selected`, or `None` to ask for no certificate.
    fn request(&self, selected: &SelectedTarget<T>) -> Option<Self::Request>;
}

/// Asks no client for a certificate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NoCertificateRequest;

impl<T> PeerAuthentication<T> for NoCertificateRequest {
    type Request = Infallible;

    fn request(&self, _selected: &SelectedTarget<T>) -> Option<Infallible> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{NoCertificateRequest, PeerAuthentication};
    use crate::server::targets::SelectedTarget;

    #[test]
    fn no_certificate_is_asked_for() {
        let selected = SelectedTarget {
            target: (),
            application_protocol: None,
        };
        assert!(NoCertificateRequest.request(&selected).is_none());
    }
}

//! The capabilities a client's handshake consults, with implementations
//! for plain Web connections, and the client's handshake engine.
//!
//! - [`offering`]: what the ClientHello offers: cipher suites, groups and
//!   key shares, signature schemes.
//! - [`ticket`]: the tickets a client keeps, per target.
//! - [`identity`]: what the client presents when the server asks for its
//!   certificate.
//!
//! [`engine`] is the client's handshake engine, which takes these
//! capabilities and a certificate verifier from
//! [`verify`](crate::verify).
//!
//! A client connects to a [`ServerTarget`]; the capabilities answer per
//! target.

use rustls_pki_types::DnsName;
use snafu::Snafu;

pub mod engine;
pub mod identity;
pub mod offering;
pub mod ticket;

/// The server a client connects to: a DNS host name, which the ClientHello
/// sends in `server_name` and the server's certificate must be valid for.
///
/// The name is kept in lowercase, so that two spellings of one name are
/// one target.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerTarget {
    name: DnsName<'static>,
}

impl ServerTarget {
    /// The target with the DNS host name `name`.
    ///
    /// # Errors
    ///
    /// [`InvalidHostNameError`] when `name` is not a syntactically valid DNS
    /// name, such as an IP address or a name with an empty label.
    pub fn from_host_name(name: &str) -> Result<Self, InvalidHostNameError> {
        let parsed = DnsName::try_from(name).map_err(|_| InvalidHostNameError {
            name: name.to_owned(),
        })?;
        Ok(Self {
            name: parsed.to_lowercase_owned(),
        })
    }

    /// The host name, in lowercase.
    pub fn host_name(&self) -> &str {
        self.name.as_ref()
    }

    pub(crate) fn dns_name(&self) -> &DnsName<'static> {
        &self.name
    }
}

/// A string is not a valid DNS host name.
#[derive(Clone, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("{name:?} is not a valid DNS host name"))]
pub struct InvalidHostNameError {
    name: String,
}

impl InvalidHostNameError {
    /// The string that was given.
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::ServerTarget;

    #[test]
    fn a_target_is_a_lowercase_host_name() {
        let target = ServerTarget::from_host_name("Example.COM").unwrap();
        assert_eq!(target.host_name(), "example.com");
        assert_eq!(target, ServerTarget::from_host_name("example.com").unwrap());
        let error = ServerTarget::from_host_name("192.0.2.1").unwrap_err();
        assert_eq!(error.name(), "192.0.2.1");
        assert!(ServerTarget::from_host_name("a..b").is_err());
    }
}

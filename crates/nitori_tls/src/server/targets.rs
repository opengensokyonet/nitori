//! The target a ClientHello asks for.
//!
//! A server may serve several targets, each with its own application
//! protocols. [`TargetSelection::select`] picks the target of a ClientHello
//! and negotiates its application protocol; [`SniTargets`] picks by the
//! `server_name` extension. [`PreselectedTarget`] hands over a target
//! selected before the handshake began, such as by a server that decided
//! what to do with the connection.

use crate::{
    alert::AlertDescription, extension::alpn::ProtocolName, message::client_hello::ClientHello,
};
use bytes::Bytes;
use core::{
    convert::Infallible,
    future::{Future, Ready, ready},
};
use snafu::Snafu;
use std::collections::HashMap;

/// A selected target: what the server serves on this connection, with the
/// application protocol negotiated for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedTarget<T> {
    /// The target.
    pub target: T,
    /// The application protocol, `None` when the connection negotiates
    /// none.
    pub application_protocol: Option<ProtocolName<Bytes>>,
}

/// How a server picks the target of a ClientHello.
pub trait TargetSelection {
    /// What the server serves, with the configuration of the application.
    type Target;

    /// A failure to select.
    type Error;

    /// Selection in progress.
    type Selecting<'a>: Future<Output = Result<SelectedTarget<Self::Target>, Self::Error>> + 'a
    where
        Self: 'a;

    /// Selects the target of `hello` and its application protocol.
    fn select<'a>(&'a self, hello: &'a ClientHello<Bytes>) -> Self::Selecting<'a>;
}

/// One target of [`SniTargets`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct SniEntry<T> {
    target: T,
    protocols: Vec<ProtocolName<Bytes>>,
}

/// Targets by host name, matched against `server_name` without regard to
/// ASCII case, with an optional fallback for a ClientHello without a name
/// or with a name of no target.
///
/// Each target has the application protocols it serves, most preferred
/// first. The protocol is the first of them the client offers; a target
/// with no protocols negotiates none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SniTargets<T> {
    names: HashMap<Vec<u8>, SniEntry<T>>,
    fallback: Option<SniEntry<T>>,
}

impl<T> Default for SniTargets<T> {
    fn default() -> Self {
        Self {
            names: HashMap::new(),
            fallback: None,
        }
    }
}

impl<T> SniTargets<T> {
    /// No targets.
    pub fn new() -> Self {
        Self::default()
    }

    /// Serves `target` for `host_name` with `protocols`, replacing an
    /// earlier target of the name.
    pub fn insert(&mut self, host_name: &str, target: T, protocols: Vec<ProtocolName<Bytes>>) {
        self.names.insert(
            host_name.as_bytes().to_ascii_lowercase(),
            SniEntry { target, protocols },
        );
    }

    /// Serves `target` with `protocols` to a ClientHello without a name or
    /// with a name of no target.
    pub fn set_fallback(&mut self, target: T, protocols: Vec<ProtocolName<Bytes>>) {
        self.fallback = Some(SniEntry { target, protocols });
    }
}

impl<T: Clone> SniTargets<T> {
    /// Selects for `hello` at once.
    ///
    /// # Errors
    ///
    /// [`SelectTargetError`].
    pub fn select_now<C: AsRef<[u8]>>(
        &self,
        hello: &ClientHello<C>,
    ) -> Result<SelectedTarget<T>, SelectTargetError> {
        let entry = match hello.server_name() {
            Some(name) => {
                let name = name.host_name().as_ref();
                self.names
                    .get(&name.to_ascii_lowercase())
                    .or(self.fallback.as_ref())
                    .ok_or_else(|| SelectTargetError::UnknownServerName {
                        name: Bytes::copy_from_slice(name),
                    })?
            }
            None => self
                .fallback
                .as_ref()
                .ok_or(SelectTargetError::NoServerName)?,
        };
        let application_protocol = if entry.protocols.is_empty() {
            None
        } else {
            let offered = hello
                .application_layer_protocol_negotiation()
                .map_or(&[][..], |list| list.names());
            let protocol = entry
                .protocols
                .iter()
                .find(|ours| {
                    offered
                        .iter()
                        .any(|theirs| theirs.name().as_ref() == ours.name().as_ref())
                })
                .ok_or(SelectTargetError::NoCommonProtocol)?;
            Some(protocol.clone())
        };
        Ok(SelectedTarget {
            target: entry.target.clone(),
            application_protocol,
        })
    }
}

impl<T: Clone> TargetSelection for SniTargets<T> {
    type Target = T;
    type Error = SelectTargetError;
    type Selecting<'a>
        = Ready<Result<SelectedTarget<T>, SelectTargetError>>
    where
        Self: 'a;

    fn select<'a>(&'a self, hello: &'a ClientHello<Bytes>) -> Self::Selecting<'a> {
        ready(self.select_now(hello))
    }
}

/// A target selected before the handshake began, which every selection
/// hands over whatever the ClientHello.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreselectedTarget<T> {
    selected: SelectedTarget<T>,
}

impl<T> PreselectedTarget<T> {
    /// A selection that hands over `selected`.
    pub const fn new(selected: SelectedTarget<T>) -> Self {
        Self { selected }
    }
}

impl<T: Clone> TargetSelection for PreselectedTarget<T> {
    type Target = T;
    type Error = Infallible;
    type Selecting<'a>
        = Ready<Result<SelectedTarget<T>, Infallible>>
    where
        Self: 'a;

    fn select<'a>(&'a self, _hello: &'a ClientHello<Bytes>) -> Self::Selecting<'a> {
        ready(Ok(self.selected.clone()))
    }
}

/// [`SniTargets`] has no target for a ClientHello.
#[derive(Clone, Debug, PartialEq, Eq, Snafu)]
pub enum SelectTargetError {
    /// The ClientHello has no `server_name`, and there is no fallback.
    #[snafu(display("ClientHello names no server"))]
    NoServerName,
    /// No target has the name, and there is no fallback.
    #[snafu(display("no target for server name {name:?}"))]
    UnknownServerName {
        /// The host name the client sent.
        name: Bytes,
    },
    /// The target serves application protocols, none of which the client
    /// offers.
    #[snafu(display("no common application protocol"))]
    NoCommonProtocol,
}

impl SelectTargetError {
    /// The alert the failure calls for: unrecognized_name without a target
    /// (RFC 6066 §3), no_application_protocol without a common protocol
    /// (RFC 7301 §3.2).
    pub fn alert(&self) -> AlertDescription {
        match self {
            Self::NoServerName | Self::UnknownServerName { .. } => {
                AlertDescription::UNRECOGNIZED_NAME
            }
            Self::NoCommonProtocol => AlertDescription::NO_APPLICATION_PROTOCOL,
        }
    }
}

impl crate::alert::CallsForAlert for SelectTargetError {
    fn alert(&self) -> AlertDescription {
        SelectTargetError::alert(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PreselectedTarget, SelectTargetError, SelectedTarget, SniTargets, TargetSelection,
    };
    use crate::{
        alert::AlertDescription,
        extension::alpn::ProtocolName,
        test_support::{
            VERSIONS, client_hello, concat, extension, ready_now, received_client_hello, vector,
        },
    };
    use bytes::Bytes;

    fn protocol(name: &'static [u8]) -> ProtocolName<Bytes> {
        ProtocolName::new(Bytes::from_static(name)).unwrap()
    }

    fn hello(name: Option<&[u8]>, protocols: &[&[u8]]) -> Vec<u8> {
        let mut extensions = extension(43, VERSIONS);
        if let Some(name) = name {
            let entry = concat(&[&[0], &vector(2, name)]);
            extensions.extend(extension(0, &vector(2, &entry)));
        }
        if !protocols.is_empty() {
            let list: Vec<u8> = protocols
                .iter()
                .flat_map(|protocol| vector(1, protocol))
                .collect();
            extensions.extend(extension(16, &vector(2, &list)));
        }
        client_hello(&extensions)
    }

    fn targets() -> SniTargets<&'static str> {
        let mut targets = SniTargets::new();
        targets.insert(
            "Example.com",
            "web",
            vec![protocol(b"h3"), protocol(b"h3-29")],
        );
        targets.insert("raw.example.com", "raw", vec![]);
        targets
    }

    fn select(
        targets: &SniTargets<&'static str>,
        bytes: &[u8],
    ) -> Result<SelectedTarget<&'static str>, SelectTargetError> {
        let (_, hello) = received_client_hello(bytes);
        ready_now(targets.select(&hello))
    }

    #[test]
    fn the_name_picks_the_target_and_its_protocol() {
        let selected =
            select(&targets(), &hello(Some(b"EXAMPLE.com"), &[b"h3-29", b"h3"])).unwrap();
        assert_eq!(selected.target, "web");
        // The server's order wins.
        assert_eq!(selected.application_protocol, Some(protocol(b"h3")));
        let selected = select(&targets(), &hello(Some(b"raw.example.com"), &[])).unwrap();
        assert_eq!(
            selected,
            SelectedTarget {
                target: "raw",
                application_protocol: None
            }
        );
    }

    #[test]
    fn without_a_common_protocol_selection_fails() {
        for protocols in [&[][..], &[&b"h2"[..]]] {
            let error = select(&targets(), &hello(Some(b"example.com"), protocols)).unwrap_err();
            assert_eq!(error, SelectTargetError::NoCommonProtocol);
            assert_eq!(error.alert(), AlertDescription::NO_APPLICATION_PROTOCOL);
        }
    }

    #[test]
    fn an_unknown_or_missing_name_takes_the_fallback() {
        let error = select(&targets(), &hello(Some(b"other.example"), &[b"h3"])).unwrap_err();
        assert_eq!(
            error,
            SelectTargetError::UnknownServerName {
                name: Bytes::from_static(b"other.example")
            }
        );
        assert_eq!(error.alert(), AlertDescription::UNRECOGNIZED_NAME);
        assert_eq!(
            select(&targets(), &hello(None, &[b"h3"])).unwrap_err(),
            SelectTargetError::NoServerName
        );
        let mut targets = targets();
        targets.set_fallback("default", vec![]);
        assert_eq!(
            select(&targets, &hello(Some(b"other.example"), &[]))
                .unwrap()
                .target,
            "default"
        );
        assert_eq!(
            select(&targets, &hello(None, &[])).unwrap().target,
            "default"
        );
    }

    #[test]
    fn a_preselected_target_is_handed_over_for_any_client_hello() {
        let selected = SelectedTarget {
            target: "web",
            application_protocol: Some(protocol(b"h3")),
        };
        let targets = PreselectedTarget::new(selected.clone());
        let (_, hello) = received_client_hello(&client_hello(&extension(43, VERSIONS)));
        let Ok(handed) = ready_now(targets.select(&hello));
        assert_eq!(handed, selected);
    }
}

use core::fmt;

use smallvec::SmallVec;
use snafu::Snafu;

use crate::{Issuer, Secret, Supported};

/// A TLS NamedGroup code point (RFC 8446 §4.2.7): a key exchange group.
///
/// Any 16-bit value is a code point; the constants name the groups that
/// implementations in this family provide.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NamedGroup(u16);

impl NamedGroup {
    /// secp256r1, ECDH over P-256.
    pub const SECP256R1: Self = Self(0x0017);
    /// secp384r1, ECDH over P-384.
    pub const SECP384R1: Self = Self(0x0018);
    /// secp521r1, ECDH over P-521.
    pub const SECP521R1: Self = Self(0x0019);
    /// x25519, ECDH over Curve25519 (RFC 7748).
    pub const X25519: Self = Self(0x001d);
    /// MLKEM768, the ML-KEM-768 key encapsulation mechanism (FIPS 203).
    pub const MLKEM768: Self = Self(0x0201);
    /// X25519MLKEM768, ML-KEM-768 combined with X25519 (RFC 10024).
    pub const X25519MLKEM768: Self = Self(0x11ec);

    /// The group with code point `value`.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// The code point.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }

    const fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::SECP256R1 => "secp256r1",
            Self::SECP384R1 => "secp384r1",
            Self::SECP521R1 => "secp521r1",
            Self::X25519 => "x25519",
            Self::MLKEM768 => "MLKEM768",
            Self::X25519MLKEM768 => "X25519MLKEM768",
            _ => return None,
        })
    }
}

impl fmt::Debug for NamedGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "NamedGroup({name})"),
            None => write!(f, "NamedGroup({:#06x})", self.0),
        }
    }
}

/// One side's public value in a key exchange: the `key_exchange` field of a
/// TLS KeyShareEntry.
///
/// Shares of the classical groups are stored inline, up to the 97 bytes of
/// an uncompressed P-384 point; longer shares, such as those of hybrid
/// groups, are stored on the heap.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct KeyShare(SmallVec<[u8; KeyShare::INLINE_CAPACITY]>);

impl KeyShare {
    const INLINE_CAPACITY: usize = 97;

    /// Copy `bytes` into a new share.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self(SmallVec::from_slice(bytes))
    }

    /// Append `bytes` to the share.
    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    /// The share bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for KeyShare {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for KeyShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyShare(")?;
        for byte in self.as_bytes() {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

/// A key exchange a client started and has not completed.
///
/// [`StartedExchange::complete`] consumes it, so its private key is used at
/// most once.
pub trait StartedExchange {
    /// The group of the exchange.
    fn group(&self) -> NamedGroup;

    /// The client's share, to send in the ClientHello `key_share`.
    fn share(&self) -> &KeyShare;

    /// The shared secret, from the server's share.
    ///
    /// # Errors
    ///
    /// Fails when the server's share is not a valid share of the group, or
    /// when a KEM group's decapsulation fails for another reason.
    fn complete(self, server_share: &[u8]) -> Result<Secret, CompleteError>;
}

/// Key exchange for TLS 1.3, by NamedGroup.
///
/// The server takes one step: for an ECDH group it generates a key pair and
/// computes the shared value; for a KEM group it encapsulates to the
/// client's public key. The client takes two: [`KeyExchange::start`] gives
/// the share to send, and [`StartedExchange::complete`] takes the server's
/// share.
pub trait KeyExchange: Issuer {
    /// An exchange started by [`KeyExchange::start`].
    type Started: StartedExchange;

    /// Every group for which [`KeyExchange::named_group`] issues a
    /// [`Supported`].
    fn named_groups(&self) -> &[NamedGroup];

    /// Whether this implementation supports `group`.
    fn named_group(&self, group: NamedGroup) -> Option<Supported<Self, NamedGroup>>;

    /// The server's step: its share and the shared secret, from the
    /// client's share.
    ///
    /// # Errors
    ///
    /// Fails when the server's key cannot be generated or the client's share
    /// is not a valid share of the group.
    fn server_share(
        &self,
        group: Supported<Self, NamedGroup>,
        client_share: &[u8],
    ) -> Result<(KeyShare, Secret), ServerShareError>;

    /// The client's first step: a new private key and its share.
    ///
    /// # Errors
    ///
    /// Fails when the key cannot be generated.
    fn start(
        &self,
        group: Supported<Self, NamedGroup>,
    ) -> Result<Self::Started, KeyGenerationFailedError>;
}

/// A key exchange's private key could not be generated.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
#[snafu(display("key generation for {group:?} failed"))]
pub struct KeyGenerationFailedError {
    /// The group of the key.
    pub group: NamedGroup,
}

/// The peer's share is not a valid share of the group.
///
/// This covers a share of the wrong length, a point not on the curve, an
/// X25519 share whose shared secret is all zeros (RFC 8446 §7.4.2), and an
/// ML-KEM encapsulation key that fails the check of FIPS 203 §7.2.
///
/// The cryptography libraries behind the backends report only one error
/// without details for all of these cases, so this type does not tell them
/// apart.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
#[snafu(display("peer share for {group:?} is invalid"))]
pub struct InvalidPeerShareError {
    /// The group of the exchange.
    pub group: NamedGroup,
}

/// ML-KEM decapsulation of a ciphertext of the right length failed.
///
/// RFC 10024 §4.2 separates this from an invalid share: it is an internal
/// error of the client, not an error of the server's share.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
#[snafu(display("decapsulation for {group:?} failed"))]
pub struct DecapsulationFailedError {
    /// The group of the exchange.
    pub group: NamedGroup,
}

/// The server's step of a key exchange failed.
#[derive(Debug, Snafu)]
pub enum ServerShareError {
    /// The server's key could not be generated.
    #[snafu(transparent)]
    KeyGeneration {
        /// The failure.
        source: KeyGenerationFailedError,
    },
    /// The client's share is invalid.
    #[snafu(transparent)]
    InvalidPeerShare {
        /// The failure.
        source: InvalidPeerShareError,
    },
}

/// The client's completion of a key exchange failed.
#[derive(Debug, Snafu)]
pub enum CompleteError {
    /// The server's share is invalid.
    #[snafu(transparent)]
    InvalidPeerShare {
        /// The failure.
        source: InvalidPeerShareError,
    },
    /// Decapsulation failed for a reason other than the share's length.
    #[snafu(transparent)]
    Decapsulation {
        /// The failure.
        source: DecapsulationFailedError,
    },
}

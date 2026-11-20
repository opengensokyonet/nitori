//! What a client offers in its ClientHello.
//!
//! Before it writes a ClientHello, a client decides which cipher suites to
//! offer and in which order, which groups to list in `supported_groups`,
//! which of them carry a key share, which signature schemes to list and
//! whether to ask for SCTs. An [`Offering`] decides; [`PreferenceOffering`]
//! decides by fixed preference orders and remembers the group each server
//! asked for with a HelloRetryRequest.
//!
//! An offer is one interaction: [`Offering::offer`] returns the [`Offer`]
//! with an [`OfferAttempt`], which the handshake consumes with the group of
//! a HelloRetryRequest, or drops when there was none.

use crate::{
    cipher_suite::{CipherSuite, SupportedSuite},
    client::ServerTarget,
};
use alloc::{vec, vec::Vec};
use core::{fmt, hash::BuildHasher};
use hashbrown::HashMap;
use nitori_crypto::{Hash, KeyExchange, NamedGroup, SignatureScheme, SignatureVerify, Supported};
use nitori_sync::{SharedState as _, Synchronization};
use snafu::{Snafu, ensure};

/// What a certificate verifier accepts, which the ClientHello states.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VerifierSupport {
    signature_schemes: Vec<SignatureScheme>,
    signed_certificate_timestamps: bool,
}

impl VerifierSupport {
    /// A verifier that accepts `signature_schemes`, most preferred first,
    /// and wants SCTs from the server in the handshake when
    /// `signed_certificate_timestamps`.
    pub fn new(
        signature_schemes: Vec<SignatureScheme>,
        signed_certificate_timestamps: bool,
    ) -> Self {
        Self {
            signature_schemes,
            signed_certificate_timestamps,
        }
    }

    /// The signature schemes, most preferred first.
    pub fn signature_schemes(&self) -> &[SignatureScheme] {
        &self.signature_schemes
    }

    /// Whether the verifier wants SCTs from the server in the handshake.
    pub fn signed_certificate_timestamps(&self) -> bool {
        self.signed_certificate_timestamps
    }
}

/// What one ClientHello offers, every item supported by the backend `C`.
///
/// The groups with a key share are among the listed groups.
pub struct Offer<C> {
    suites: Vec<SupportedSuite<C>>,
    groups: Vec<Supported<C, NamedGroup>>,
    key_share_groups: Vec<Supported<C, NamedGroup>>,
    signature_schemes: Vec<Supported<C, SignatureScheme>>,
    signed_certificate_timestamps: bool,
}

impl<C> Offer<C> {
    /// The offer of `suites`, `groups` with key shares for
    /// `key_share_groups`, and `signature_schemes`, each most preferred
    /// first, asking for SCTs when `signed_certificate_timestamps`.
    ///
    /// # Errors
    ///
    /// [`InvalidOfferError`] when a list a ClientHello cannot send empty is
    /// empty, or a key share is not of a listed group or not the only one
    /// of its group.
    pub fn new(
        suites: Vec<SupportedSuite<C>>,
        groups: Vec<Supported<C, NamedGroup>>,
        key_share_groups: Vec<Supported<C, NamedGroup>>,
        signature_schemes: Vec<Supported<C, SignatureScheme>>,
        signed_certificate_timestamps: bool,
    ) -> Result<Self, InvalidOfferError> {
        use invalid_offer::*;
        ensure!(!suites.is_empty(), NoSuiteSnafu);
        ensure!(!groups.is_empty(), NoGroupSnafu);
        ensure!(!signature_schemes.is_empty(), NoSignatureSchemeSnafu);
        for (index, share) in key_share_groups.iter().enumerate() {
            let group = share.algorithm();
            ensure!(groups.contains(share), KeyShareNotListedSnafu { group });
            ensure!(
                !key_share_groups[..index].contains(share),
                DuplicateKeyShareSnafu { group }
            );
        }
        Ok(Self {
            suites,
            groups,
            key_share_groups,
            signature_schemes,
            signed_certificate_timestamps,
        })
    }

    /// The cipher suites, most preferred first.
    pub fn suites(&self) -> &[SupportedSuite<C>] {
        &self.suites
    }

    /// The groups of `supported_groups`, most preferred first.
    pub fn groups(&self) -> &[Supported<C, NamedGroup>] {
        &self.groups
    }

    /// The groups that carry a key share, a subset of
    /// [`groups`](Self::groups).
    pub fn key_share_groups(&self) -> &[Supported<C, NamedGroup>] {
        &self.key_share_groups
    }

    /// The schemes of `signature_algorithms`, most preferred first.
    pub fn signature_schemes(&self) -> &[Supported<C, SignatureScheme>] {
        &self.signature_schemes
    }

    /// Whether the ClientHello asks for SCTs with
    /// `signed_certificate_timestamp`.
    pub fn signed_certificate_timestamps(&self) -> bool {
        self.signed_certificate_timestamps
    }
}

impl<C> core::fmt::Debug for Offer<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Offer")
            .field("suites", &self.suites)
            .field("groups", &self.groups)
            .field("key_share_groups", &self.key_share_groups)
            .field("signature_schemes", &self.signature_schemes)
            .field(
                "signed_certificate_timestamps",
                &self.signed_certificate_timestamps,
            )
            .finish()
    }
}

/// Failures of [`Offer::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(module(invalid_offer), visibility(pub(crate)))]
pub enum InvalidOfferError {
    /// No cipher suite: `cipher_suites` cannot be empty (RFC 9846 §4.2.2).
    #[snafu(display("offer has no cipher suite"))]
    NoSuite,
    /// No group: `supported_groups` cannot be empty (RFC 9846 §4.3.7).
    #[snafu(display("offer has no key exchange group"))]
    NoGroup,
    /// No signature scheme: `signature_algorithms` cannot be empty
    /// (RFC 9846 §4.3.3).
    #[snafu(display("offer has no signature scheme"))]
    NoSignatureScheme,
    /// A key share is of a group the offer does not list
    /// (RFC 9846 §4.3.8).
    #[snafu(display("key share of {group:?}, which is not offered"))]
    KeyShareNotListed {
        /// The group.
        group: NamedGroup,
    },
    /// Two key shares are of one group (RFC 9846 §4.3.8).
    #[snafu(display("more than one key share of {group:?}"))]
    DuplicateKeyShare {
        /// The group.
        group: NamedGroup,
    },
}

/// How a client decides what its ClientHello offers.
///
/// The decision may use what the backend and the verifier support, the
/// target, and what the implementation remembers of earlier handshakes with
/// the target, such as the group the server asked for. State across
/// connections belongs to the implementation, which shares it internally;
/// the handshake holds only the value.
pub trait Offering<C> {
    /// The interaction one offer begins.
    type Attempt: OfferAttempt<C>;

    /// What to offer to `target` on `backend`, with a verifier that accepts
    /// `verifier`.
    ///
    /// # Errors
    ///
    /// [`InvalidOfferError`] when the backend and the verifier leave nothing to
    /// offer.
    fn offer(
        &self,
        backend: &C,
        target: &ServerTarget,
        verifier: &VerifierSupport,
    ) -> Result<(Offer<C>, Self::Attempt), InvalidOfferError>;
}

/// The outcome of one offer.
///
/// Dropping the value means the server did not send a HelloRetryRequest.
pub trait OfferAttempt<C> {
    /// The server asked for a key share of `group` with a
    /// HelloRetryRequest.
    fn retried(self, group: Supported<C, NamedGroup>);
}

/// Offers by fixed preference orders, and remembers per target the group a
/// HelloRetryRequest asked for.
///
/// It offers the suites, groups and signature schemes of its tables that
/// the backend supports, the last restricted to those the verifier
/// accepts. Key shares go to the remembered group of the target, or to the
/// configured key share groups that the backend supports, or, when none
/// is, to the first offered group. Clones share the memory, which holds at
/// most [`PreferenceOffering::MEMORY_CAPACITY`] targets.
///
/// The memory is shared state of the family `Y`, its targets hashed by `S`.
pub struct PreferenceOffering<Y: Synchronization, S> {
    suites: Vec<CipherSuite>,
    groups: Vec<NamedGroup>,
    key_share_groups: Vec<NamedGroup>,
    retried: Y::SharedState<HashMap<ServerTarget, NamedGroup, S>>,
}

impl<Y: Synchronization, S> Clone for PreferenceOffering<Y, S> {
    fn clone(&self) -> Self {
        Self {
            suites: self.suites.clone(),
            groups: self.groups.clone(),
            key_share_groups: self.key_share_groups.clone(),
            retried: self.retried.clone(),
        }
    }
}

impl<Y: Synchronization, S> fmt::Debug for PreferenceOffering<Y, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreferenceOffering")
            .field("suites", &self.suites)
            .field("groups", &self.groups)
            .field("key_share_groups", &self.key_share_groups)
            .finish_non_exhaustive()
    }
}

impl<Y: Synchronization, S> PreferenceOffering<Y, S> {
    /// The most targets whose retried group the memory holds; past it, a
    /// new target replaces an arbitrary one.
    pub const MEMORY_CAPACITY: usize = 1024;
}

impl<Y: Synchronization, S: BuildHasher + Default> PreferenceOffering<Y, S> {
    /// Offers `suites` and `groups`, most preferred first, with key shares
    /// for `key_share_groups`.
    ///
    /// # Errors
    ///
    /// [`KeyShareNotListedError`] for a key share group that is not in `groups`.
    pub fn new(
        suites: Vec<CipherSuite>,
        groups: Vec<NamedGroup>,
        key_share_groups: Vec<NamedGroup>,
    ) -> Result<Self, KeyShareNotListedError> {
        if let Some(&group) = key_share_groups
            .iter()
            .find(|group| !groups.contains(group))
        {
            return Err(KeyShareNotListedError { group });
        }
        Ok(Self {
            suites,
            groups,
            key_share_groups,
            retried: Y::share(HashMap::default()),
        })
    }
}

impl<Y: Synchronization, S: BuildHasher> PreferenceOffering<Y, S> {
    fn remembered(&self, target: &ServerTarget) -> Option<NamedGroup> {
        self.retried.with(|memory| memory.get(target).copied())
    }
}

/// A key share group of a [`PreferenceOffering`] is not among its groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("key share group {group:?} is not among the offered groups"))]
pub struct KeyShareNotListedError {
    group: NamedGroup,
}

impl KeyShareNotListedError {
    /// The group.
    pub fn group(self) -> NamedGroup {
        self.group
    }
}

impl<C, Y, S> Offering<C> for PreferenceOffering<Y, S>
where
    C: Hash + KeyExchange + SignatureVerify,
    Y: Synchronization,
    S: BuildHasher,
{
    type Attempt = PreferenceAttempt<Y, S>;

    fn offer(
        &self,
        backend: &C,
        target: &ServerTarget,
        verifier: &VerifierSupport,
    ) -> Result<(Offer<C>, PreferenceAttempt<Y, S>), InvalidOfferError> {
        let suites = self
            .suites
            .iter()
            .filter_map(|&suite| SupportedSuite::new(backend, suite))
            .collect();
        let groups: Vec<_> = self
            .groups
            .iter()
            .filter_map(|&group| backend.named_group(group))
            .collect();
        let remembered = self
            .remembered(target)
            .and_then(|group| groups.iter().find(|offered| offered.algorithm() == group));
        let key_share_groups = match remembered {
            Some(&group) => vec![group],
            None => {
                let configured: Vec<_> = groups
                    .iter()
                    .filter(|group| self.key_share_groups.contains(&group.algorithm()))
                    .copied()
                    .collect();
                if configured.is_empty() {
                    groups.first().copied().into_iter().collect()
                } else {
                    configured
                }
            }
        };
        let signature_schemes = verifier
            .signature_schemes()
            .iter()
            .filter_map(|&scheme| backend.signature_scheme(scheme))
            .collect();
        let offer = Offer::new(
            suites,
            groups,
            key_share_groups,
            signature_schemes,
            verifier.signed_certificate_timestamps(),
        )?;
        let attempt = PreferenceAttempt {
            retried: self.retried.clone(),
            target: target.clone(),
        };
        Ok((offer, attempt))
    }
}

/// One offer of a [`PreferenceOffering`]: a HelloRetryRequest it reports
/// becomes the target's remembered group.
pub struct PreferenceAttempt<Y: Synchronization, S> {
    retried: Y::SharedState<HashMap<ServerTarget, NamedGroup, S>>,
    target: ServerTarget,
}

impl<Y: Synchronization, S> fmt::Debug for PreferenceAttempt<Y, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreferenceAttempt")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl<C, Y: Synchronization, S: BuildHasher> OfferAttempt<C> for PreferenceAttempt<Y, S> {
    fn retried(self, group: Supported<C, NamedGroup>) {
        let Self { retried, target } = self;
        retried.with(|memory| {
            if memory.len() >= PreferenceOffering::<Y, S>::MEMORY_CAPACITY
                && !memory.contains_key(&target)
                && let Some(evicted) = memory.keys().next().cloned()
            {
                memory.remove(&evicted);
            }
            memory.insert(target, group.algorithm());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InvalidOfferError, KeyShareNotListedError, Offer, OfferAttempt, Offering,
        PreferenceOffering, VerifierSupport,
    };
    use crate::{
        cipher_suite::{CipherSuite, SupportedSuite},
        client::ServerTarget,
    };
    use nitori_crypto::{KeyExchange, NamedGroup, SignatureScheme, SignatureVerify};
    use nitori_crypto_aws_lc::AwsLcBackend;
    use nitori_sync::RefCellSynchronization;
    use std::hash::RandomState;

    type Preferences = PreferenceOffering<RefCellSynchronization, RandomState>;

    fn offering() -> Preferences {
        Preferences::new(
            vec![
                CipherSuite::TLS_AES_128_GCM_SHA256,
                CipherSuite::TLS_AES_128_CCM_SHA256,
                CipherSuite::TLS_CHACHA20_POLY1305_SHA256,
            ],
            vec![
                NamedGroup::X25519MLKEM768,
                NamedGroup::X25519,
                NamedGroup::new(0x9999),
                NamedGroup::SECP256R1,
            ],
            vec![NamedGroup::X25519],
        )
        .unwrap()
    }

    fn verifier() -> VerifierSupport {
        VerifierSupport::new(
            vec![
                SignatureScheme::ECDSA_SECP256R1_SHA256,
                SignatureScheme::new(0x0201),
                SignatureScheme::RSA_PSS_RSAE_SHA256,
            ],
            false,
        )
    }

    fn algorithms<C, A: Copy>(items: &[nitori_crypto::Supported<C, A>]) -> Vec<A> {
        items.iter().map(|item| item.algorithm()).collect()
    }

    #[test]
    fn the_offer_holds_what_the_backend_supports() {
        let target = ServerTarget::from_host_name("example.com").unwrap();
        let (offer, _attempt) =
            Offering::<AwsLcBackend>::offer(&offering(), &AwsLcBackend, &target, &verifier())
                .unwrap();
        let suites: Vec<_> = offer.suites().iter().map(SupportedSuite::suite).collect();
        assert_eq!(
            suites,
            [
                CipherSuite::TLS_AES_128_GCM_SHA256,
                CipherSuite::TLS_CHACHA20_POLY1305_SHA256
            ]
        );
        assert_eq!(
            algorithms(offer.groups()),
            [
                NamedGroup::X25519MLKEM768,
                NamedGroup::X25519,
                NamedGroup::SECP256R1
            ]
        );
        assert_eq!(algorithms(offer.key_share_groups()), [NamedGroup::X25519]);
        assert_eq!(
            algorithms(offer.signature_schemes()),
            [
                SignatureScheme::ECDSA_SECP256R1_SHA256,
                SignatureScheme::RSA_PSS_RSAE_SHA256
            ]
        );
        assert!(!offer.signed_certificate_timestamps());
    }

    #[test]
    fn a_retry_moves_the_key_share_for_that_target() {
        let offering = offering();
        let target = ServerTarget::from_host_name("example.com").unwrap();
        let other = ServerTarget::from_host_name("example.net").unwrap();
        let backend = AwsLcBackend;
        let (_, attempt) = offering.offer(&backend, &target, &verifier()).unwrap();
        attempt.retried(backend.named_group(NamedGroup::SECP256R1).unwrap());
        let (offer, attempt) = offering.offer(&backend, &target, &verifier()).unwrap();
        assert_eq!(
            algorithms(offer.key_share_groups()),
            [NamedGroup::SECP256R1]
        );
        // No retry this time: the memory stays.
        drop::<super::PreferenceAttempt<RefCellSynchronization, RandomState>>(attempt);
        let (offer, _) = offering
            .clone()
            .offer(&backend, &target, &verifier())
            .unwrap();
        assert_eq!(
            algorithms(offer.key_share_groups()),
            [NamedGroup::SECP256R1]
        );
        let (offer, _) = offering.offer(&backend, &other, &verifier()).unwrap();
        assert_eq!(algorithms(offer.key_share_groups()), [NamedGroup::X25519]);
    }

    #[test]
    fn key_shares_are_of_listed_groups() {
        assert_eq!(
            Preferences::new(
                vec![],
                vec![NamedGroup::X25519],
                vec![NamedGroup::SECP256R1]
            )
            .unwrap_err(),
            KeyShareNotListedError {
                group: NamedGroup::SECP256R1
            }
        );
        let backend = AwsLcBackend;
        let suite = SupportedSuite::new(&backend, CipherSuite::TLS_AES_128_GCM_SHA256).unwrap();
        let x25519 = backend.named_group(NamedGroup::X25519).unwrap();
        let p256 = backend.named_group(NamedGroup::SECP256R1).unwrap();
        let scheme = backend.signature_scheme(SignatureScheme::ED25519).unwrap();
        assert_eq!(
            Offer::new(vec![suite], vec![x25519], vec![p256], vec![scheme], false).unwrap_err(),
            InvalidOfferError::KeyShareNotListed {
                group: NamedGroup::SECP256R1
            }
        );
        assert_eq!(
            Offer::new(
                vec![suite],
                vec![x25519],
                vec![x25519, x25519],
                vec![scheme],
                false
            )
            .unwrap_err(),
            InvalidOfferError::DuplicateKeyShare {
                group: NamedGroup::X25519
            }
        );
        assert_eq!(
            Offer::new(vec![], vec![x25519], vec![], vec![scheme], false).unwrap_err(),
            InvalidOfferError::NoSuite
        );
        assert_eq!(
            Offer::new(vec![suite], vec![x25519], vec![], vec![], false).unwrap_err(),
            InvalidOfferError::NoSignatureScheme
        );
        assert!(Offer::new(vec![suite], vec![x25519, p256], vec![], vec![scheme], true).is_ok());
    }

    #[test]
    fn without_a_supported_key_share_group_the_first_group_carries_one() {
        let offering = Preferences::new(
            vec![CipherSuite::TLS_AES_128_GCM_SHA256],
            vec![NamedGroup::SECP384R1, NamedGroup::new(0x9999)],
            vec![NamedGroup::new(0x9999)],
        )
        .unwrap();
        let target = ServerTarget::from_host_name("example.com").unwrap();
        let (offer, _) = offering.offer(&AwsLcBackend, &target, &verifier()).unwrap();
        assert_eq!(
            algorithms(offer.key_share_groups()),
            [NamedGroup::SECP384R1]
        );
    }
}

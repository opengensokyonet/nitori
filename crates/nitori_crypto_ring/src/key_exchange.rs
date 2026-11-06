use nitori_crypto::{
    CompleteError, InvalidPeerShareError, InvalidPeerShareSnafu, KeyExchange,
    KeyGenerationFailedError, KeyGenerationFailedSnafu, KeyShare, NamedGroup, Secret,
    ServerShareError, StartedExchange, Supported,
};
use ring::{agreement, rand::SystemRandom};
use snafu::OptionExt;

use crate::{RingBackend, token::Token};

const GROUPS: &[NamedGroup] = &[
    NamedGroup::X25519,
    NamedGroup::SECP256R1,
    NamedGroup::SECP384R1,
];

fn agreement_algorithm(group: NamedGroup) -> Option<&'static agreement::Algorithm> {
    match group {
        NamedGroup::X25519 => Some(&agreement::X25519),
        NamedGroup::SECP256R1 => Some(&agreement::ECDH_P256),
        NamedGroup::SECP384R1 => Some(&agreement::ECDH_P384),
        _ => None,
    }
}

fn supported_algorithm(group: Supported<RingBackend, NamedGroup>) -> &'static agreement::Algorithm {
    // `RingBackend` issues `Supported` only for groups that have an
    // algorithm here.
    agreement_algorithm(group.algorithm()).expect("supported group has an algorithm")
}

/// A key exchange a client started with ring.
#[derive(Debug)]
pub struct RingStartedExchange {
    group: NamedGroup,
    key: agreement::EphemeralPrivateKey,
    share: KeyShare,
}

impl RingStartedExchange {
    fn generate(
        group: Supported<RingBackend, NamedGroup>,
    ) -> Result<Self, KeyGenerationFailedError> {
        let algorithm = supported_algorithm(group);
        let group = group.algorithm();
        let key = agreement::EphemeralPrivateKey::generate(algorithm, &SystemRandom::new())
            .ok()
            .context(KeyGenerationFailedSnafu { group })?;
        let public = key
            .compute_public_key()
            .ok()
            .context(KeyGenerationFailedSnafu { group })?;
        Ok(Self {
            group,
            key,
            share: KeyShare::from_slice(public.as_ref()),
        })
    }

    fn agree(self, peer_share: &[u8]) -> Result<Secret, InvalidPeerShareError> {
        let group = self.group;
        let peer = agreement::UnparsedPublicKey::new(self.key.algorithm(), peer_share);
        // ring rejects shares of the wrong length, points off the curve and
        // an all-zero X25519 result.
        agreement::agree_ephemeral(self.key, &peer, |shared| {
            // ECDH shared secrets of these groups are at most 48 bytes.
            Secret::new(shared).expect("shared secret fits in a secret")
        })
        .ok()
        .context(InvalidPeerShareSnafu { group })
    }
}

impl StartedExchange for RingStartedExchange {
    fn group(&self) -> NamedGroup {
        self.group
    }

    fn share(&self) -> &KeyShare {
        &self.share
    }

    fn complete(self, server_share: &[u8]) -> Result<Secret, CompleteError> {
        Ok(self.agree(server_share)?)
    }
}

impl KeyExchange for RingBackend {
    type Started = RingStartedExchange;

    fn named_groups(&self) -> &[NamedGroup] {
        GROUPS
    }

    fn named_group(&self, group: NamedGroup) -> Option<Supported<Self, NamedGroup>> {
        agreement_algorithm(group).map(|_| Supported::new(Token::new(), group))
    }

    fn server_share(
        &self,
        group: Supported<Self, NamedGroup>,
        client_share: &[u8],
    ) -> Result<(KeyShare, Secret), ServerShareError> {
        let started = RingStartedExchange::generate(group)?;
        let share = started.share.clone();
        Ok((share, started.agree(client_share)?))
    }

    fn start(
        &self,
        group: Supported<Self, NamedGroup>,
    ) -> Result<Self::Started, KeyGenerationFailedError> {
        RingStartedExchange::generate(group)
    }
}

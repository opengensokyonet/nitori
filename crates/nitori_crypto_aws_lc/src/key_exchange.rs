mod ecdh;
mod hybrid;
mod ml_kem;

use core::fmt;

use nitori_crypto::{
    CompleteError, KeyExchange, KeyGenerationFailedError, KeyShare, NamedGroup, Secret,
    ServerShareError, StartedExchange, Supported,
};

use crate::{AwsLcBackend, token::Token};

pub(crate) use ecdh::Ecdh;

const GROUPS: &[NamedGroup] = &[
    NamedGroup::X25519MLKEM768,
    NamedGroup::X25519,
    NamedGroup::SECP256R1,
    NamedGroup::SECP384R1,
    NamedGroup::SECP521R1,
    NamedGroup::MLKEM768,
];

/// How the backend performs the exchange of a group.
#[derive(Clone, Copy)]
enum Mechanism {
    Ecdh(&'static aws_lc_rs::agreement::Algorithm),
    MlKem768,
    X25519MlKem768,
}

fn mechanism(group: NamedGroup) -> Option<Mechanism> {
    Some(match group {
        NamedGroup::X25519 => Mechanism::Ecdh(&aws_lc_rs::agreement::X25519),
        NamedGroup::SECP256R1 => Mechanism::Ecdh(&aws_lc_rs::agreement::ECDH_P256),
        NamedGroup::SECP384R1 => Mechanism::Ecdh(&aws_lc_rs::agreement::ECDH_P384),
        NamedGroup::SECP521R1 => Mechanism::Ecdh(&aws_lc_rs::agreement::ECDH_P521),
        NamedGroup::MLKEM768 => Mechanism::MlKem768,
        NamedGroup::X25519MLKEM768 => Mechanism::X25519MlKem768,
        _ => return None,
    })
}

fn supported_mechanism(group: Supported<AwsLcBackend, NamedGroup>) -> Mechanism {
    // `AwsLcBackend` issues `Supported` only for groups that have a
    // mechanism here.
    mechanism(group.algorithm()).expect("supported group has a mechanism")
}

/// The private state of a started exchange.
enum State {
    Ecdh(Ecdh),
    MlKem768(ml_kem::MlKem768),
    X25519MlKem768(hybrid::X25519MlKem768),
}

/// A key exchange a client started with aws-lc-rs.
pub struct AwsLcStartedExchange {
    group: NamedGroup,
    share: KeyShare,
    state: State,
}

impl AwsLcStartedExchange {
    #[cfg(feature = "fixed-x25519")]
    pub(crate) fn ecdh(group: NamedGroup, ecdh: Ecdh, share: KeyShare) -> Self {
        Self {
            group,
            share,
            state: State::Ecdh(ecdh),
        }
    }
}

impl fmt::Debug for AwsLcStartedExchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsLcStartedExchange")
            .field("group", &self.group)
            .field("share", &self.share)
            .finish_non_exhaustive()
    }
}

impl StartedExchange for AwsLcStartedExchange {
    fn group(&self) -> NamedGroup {
        self.group
    }

    fn share(&self) -> &KeyShare {
        &self.share
    }

    fn complete(self, server_share: &[u8]) -> Result<Secret, CompleteError> {
        let group = self.group;
        match self.state {
            State::Ecdh(ecdh) => Ok(ecdh.agree(group, server_share, &[])?),
            State::MlKem768(ml_kem) => ml_kem.complete(group, server_share),
            State::X25519MlKem768(hybrid) => hybrid.complete(group, server_share),
        }
    }
}

impl KeyExchange for AwsLcBackend {
    type Started = AwsLcStartedExchange;

    fn named_groups(&self) -> &[NamedGroup] {
        GROUPS
    }

    fn named_group(&self, group: NamedGroup) -> Option<Supported<Self, NamedGroup>> {
        mechanism(group).map(|_| Supported::new(Token::new(), group))
    }

    fn server_share(
        &self,
        group: Supported<Self, NamedGroup>,
        client_share: &[u8],
    ) -> Result<(KeyShare, Secret), ServerShareError> {
        let mechanism = supported_mechanism(group);
        let group = group.algorithm();
        match mechanism {
            Mechanism::Ecdh(algorithm) => {
                let (ecdh, share) = Ecdh::generate(group, algorithm)?;
                let secret = ecdh.agree(group, client_share, &[])?;
                Ok((KeyShare::from_slice(share.as_ref()), secret))
            }
            Mechanism::MlKem768 => ml_kem::server_share(group, client_share),
            Mechanism::X25519MlKem768 => hybrid::server_share(group, client_share),
        }
    }

    fn start(
        &self,
        group: Supported<Self, NamedGroup>,
    ) -> Result<Self::Started, KeyGenerationFailedError> {
        let mechanism = supported_mechanism(group);
        let group = group.algorithm();
        let (state, share) = match mechanism {
            Mechanism::Ecdh(algorithm) => {
                let (ecdh, share) = Ecdh::generate(group, algorithm)?;
                (State::Ecdh(ecdh), KeyShare::from_slice(share.as_ref()))
            }
            Mechanism::MlKem768 => {
                let (ml_kem, share) = ml_kem::MlKem768::generate(group)?;
                (State::MlKem768(ml_kem), share)
            }
            Mechanism::X25519MlKem768 => {
                let (hybrid, share) = hybrid::X25519MlKem768::generate(group)?;
                (State::X25519MlKem768(hybrid), share)
            }
        };
        Ok(AwsLcStartedExchange {
            group,
            share,
            state,
        })
    }
}

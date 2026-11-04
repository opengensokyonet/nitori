use aws_lc_rs::agreement::{PrivateKey, X25519};
use nitori_crypto::{
    Issuer, KeyExchange, KeyGenerationFailed, KeyGenerationFailedSnafu, KeyShare, NamedGroup,
    Secret, ServerShareError, Supported,
};
use snafu::OptionExt;

use crate::{AwsLcStartedExchange, key_exchange::Ecdh};

/// X25519 with a private key given at construction, for reproducing
/// published traces such as those of RFC 8448.
///
/// Every exchange, on either side, uses the same private key, so its shares
/// and secrets are predictable to anyone who knows the key. It supports
/// only the x25519 group and is available with the `fixed-x25519` feature.
#[derive(Clone)]
pub struct FixedX25519 {
    private_key: [u8; 32],
}

impl FixedX25519 {
    /// Exchanges that use `private_key`, an X25519 scalar as in RFC 7748.
    #[must_use]
    pub fn new(private_key: [u8; 32]) -> Self {
        Self { private_key }
    }

    fn ecdh(&self) -> Result<(Ecdh, KeyShare), KeyGenerationFailed> {
        let group = NamedGroup::X25519;
        let key = PrivateKey::from_private_key(&X25519, &self.private_key)
            .ok()
            .context(KeyGenerationFailedSnafu { group })?;
        let (ecdh, public) = Ecdh::from_key(group, key)?;
        Ok((ecdh, KeyShare::from_slice(public.as_ref())))
    }
}

impl core::fmt::Debug for FixedX25519 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FixedX25519").finish_non_exhaustive()
    }
}

mod token {
    /// The proof that a `Supported` value comes from `FixedX25519`.
    pub struct Token(());

    impl Token {
        pub(crate) fn new() -> Self {
            Self(())
        }
    }
}

impl Issuer for FixedX25519 {
    type Token = token::Token;
}

const GROUPS: &[NamedGroup] = &[NamedGroup::X25519];

impl KeyExchange for FixedX25519 {
    type Started = AwsLcStartedExchange;

    fn named_groups(&self) -> &[NamedGroup] {
        GROUPS
    }

    fn named_group(&self, group: NamedGroup) -> Option<Supported<Self, NamedGroup>> {
        (group == NamedGroup::X25519).then(|| Supported::new(token::Token::new(), group))
    }

    fn server_share(
        &self,
        group: Supported<Self, NamedGroup>,
        client_share: &[u8],
    ) -> Result<(KeyShare, Secret), ServerShareError> {
        let (ecdh, share) = self.ecdh()?;
        let secret = ecdh.agree(group.algorithm(), client_share, &[])?;
        Ok((share, secret))
    }

    fn start(
        &self,
        group: Supported<Self, NamedGroup>,
    ) -> Result<Self::Started, KeyGenerationFailed> {
        let (ecdh, share) = self.ecdh()?;
        Ok(AwsLcStartedExchange::ecdh(group.algorithm(), ecdh, share))
    }
}

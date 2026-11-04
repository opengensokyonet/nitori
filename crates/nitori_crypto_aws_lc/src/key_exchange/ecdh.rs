use aws_lc_rs::agreement::{self, PrivateKey, PublicKey, UnparsedPublicKey};
use nitori_crypto::{
    InvalidPeerShare, InvalidPeerShareSnafu, KeyGenerationFailed, KeyGenerationFailedSnafu,
    NamedGroup, Secret,
};
use snafu::OptionExt;

/// The private key of an ECDH exchange.
pub(crate) struct Ecdh {
    key: PrivateKey,
}

impl Ecdh {
    /// A new private key and its public key.
    pub(crate) fn generate(
        group: NamedGroup,
        algorithm: &'static agreement::Algorithm,
    ) -> Result<(Self, PublicKey), KeyGenerationFailed> {
        let key = PrivateKey::generate(algorithm)
            .ok()
            .context(KeyGenerationFailedSnafu { group })?;
        Self::from_key(group, key)
    }

    /// `key` and its public key.
    pub(crate) fn from_key(
        group: NamedGroup,
        key: PrivateKey,
    ) -> Result<(Self, PublicKey), KeyGenerationFailed> {
        let public = key
            .compute_public_key()
            .ok()
            .context(KeyGenerationFailedSnafu { group })?;
        Ok((Self { key }, public))
    }

    /// The shared secret with `peer_share`, after `prefix`.
    ///
    /// aws-lc-rs rejects shares of the wrong length, points off the curve
    /// and an all-zero X25519 result.
    pub(crate) fn agree(
        &self,
        group: NamedGroup,
        peer_share: &[u8],
        prefix: &[u8],
    ) -> Result<Secret, InvalidPeerShare> {
        agreement::agree(
            &self.key,
            UnparsedPublicKey::new(self.key.algorithm(), peer_share),
            (),
            |shared| {
                // The longest prefix is a 32-byte ML-KEM shared secret before
                // a 32-byte X25519 secret; the longest ECDH secret alone is
                // the 66 bytes of P-521. Both fit in a secret.
                Ok(Secret::from_parts(&[prefix, shared]).expect("shared secret fits in a secret"))
            },
        )
        .ok()
        .context(InvalidPeerShareSnafu { group })
    }
}

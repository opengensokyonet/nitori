use aws_lc_rs::agreement::{self, PrivateKey, PublicKey, UnparsedPublicKey};
use nitori_crypto::{
    InvalidPeerShareError, InvalidPeerShareSnafu, KeyGenerationFailedError,
    KeyGenerationFailedSnafu, NamedGroup, Secret,
};
use snafu::{OptionExt, ensure};

/// The private key of an ECDH exchange.
pub(crate) struct Ecdh {
    key: PrivateKey,
}

impl Ecdh {
    /// A new private key and its public key.
    pub(crate) fn generate(
        group: NamedGroup,
        algorithm: &'static agreement::Algorithm,
    ) -> Result<(Self, PublicKey), KeyGenerationFailedError> {
        let key = PrivateKey::generate(algorithm)
            .ok()
            .context(KeyGenerationFailedSnafu { group })?;
        Self::from_key(group, key)
    }

    /// `key` and its public key.
    pub(crate) fn from_key(
        group: NamedGroup,
        key: PrivateKey,
    ) -> Result<(Self, PublicKey), KeyGenerationFailedError> {
        let public = key
            .compute_public_key()
            .ok()
            .context(KeyGenerationFailedSnafu { group })?;
        Ok((Self { key }, public))
    }

    /// The shared secret with `peer_share`, after `prefix`.
    ///
    /// A share not of the form RFC 8446 §4.2.8.2 gives is rejected here;
    /// aws-lc-rs rejects points off the curve and an all-zero X25519
    /// result.
    pub(crate) fn agree(
        &self,
        group: NamedGroup,
        peer_share: &[u8],
        prefix: &[u8],
    ) -> Result<Secret, InvalidPeerShareError> {
        ensure!(
            is_share_form(self.key.algorithm(), peer_share),
            InvalidPeerShareSnafu { group }
        );
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

/// Whether `share` has the form RFC 8446 §4.2.8.2 gives a share of
/// `algorithm`: 32 bytes for X25519, an uncompressed point for the NIST
/// curves.
///
/// aws-lc-rs also parses a SubjectPublicKeyInfo and, on the NIST curves,
/// compressed and hybrid points, none of which TLS allows.
fn is_share_form(algorithm: &agreement::Algorithm, share: &[u8]) -> bool {
    if *algorithm == agreement::X25519 {
        return share.len() == 32;
    }
    let point_len = if *algorithm == agreement::ECDH_P256 {
        65
    } else if *algorithm == agreement::ECDH_P384 {
        97
    } else {
        // The backend creates keys only for X25519 and these three curves.
        debug_assert!(*algorithm == agreement::ECDH_P521);
        133
    };
    share.len() == point_len && share.first() == Some(&0x04)
}

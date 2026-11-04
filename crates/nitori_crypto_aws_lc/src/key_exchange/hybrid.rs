//! X25519MLKEM768 (RFC 10024).
//!
//! The client's share is the ML-KEM-768 encapsulation key followed by the
//! X25519 share (§4.1); the server's share is the ML-KEM ciphertext
//! followed by the X25519 share (§4.2); the shared secret is the ML-KEM
//! shared secret followed by the X25519 shared secret (§4.3).

use aws_lc_rs::agreement::X25519;
use nitori_crypto::{
    CompleteError, InvalidPeerShareSnafu, KeyGenerationFailed, KeyShare, NamedGroup, Secret,
    ServerShareError,
};
use snafu::ensure;

use super::{
    Ecdh,
    ml_kem::{CIPHERTEXT_LEN, ENCAPSULATION_KEY_LEN, MlKem768, encapsulate},
};

const X25519_SHARE_LEN: usize = 32;
const CLIENT_SHARE_LEN: usize = ENCAPSULATION_KEY_LEN + X25519_SHARE_LEN;
const SERVER_SHARE_LEN: usize = CIPHERTEXT_LEN + X25519_SHARE_LEN;

/// The private keys of an X25519MLKEM768 exchange.
pub(crate) struct X25519MlKem768 {
    ml_kem: MlKem768,
    x25519: Ecdh,
}

impl X25519MlKem768 {
    /// New private keys and the client's share.
    pub(super) fn generate(group: NamedGroup) -> Result<(Self, KeyShare), KeyGenerationFailed> {
        let (ml_kem, mut share) = MlKem768::generate(group)?;
        let (x25519, x25519_share) = Ecdh::generate(group, &X25519)?;
        share.extend_from_slice(x25519_share.as_ref());
        Ok((Self { ml_kem, x25519 }, share))
    }

    pub(super) fn complete(
        self,
        group: NamedGroup,
        server_share: &[u8],
    ) -> Result<Secret, CompleteError> {
        ensure!(
            server_share.len() == SERVER_SHARE_LEN,
            InvalidPeerShareSnafu { group }
        );
        let (ciphertext, x25519_share) = server_share.split_at(CIPHERTEXT_LEN);
        let ml_kem_secret = self.ml_kem.decapsulate(group, ciphertext)?;
        Ok(self
            .x25519
            .agree(group, x25519_share, ml_kem_secret.as_ref())?)
    }
}

/// The server's step: the ciphertext and the server's X25519 share, and the
/// shared secret.
pub(super) fn server_share(
    group: NamedGroup,
    client_share: &[u8],
) -> Result<(KeyShare, Secret), ServerShareError> {
    ensure!(
        client_share.len() == CLIENT_SHARE_LEN,
        InvalidPeerShareSnafu { group }
    );
    let (encapsulation_key, x25519_client) = client_share.split_at(ENCAPSULATION_KEY_LEN);
    let (ciphertext, ml_kem_secret) = encapsulate(group, encapsulation_key)?;
    let (x25519, x25519_share) = Ecdh::generate(group, &X25519)?;
    let secret = x25519.agree(group, x25519_client, ml_kem_secret.as_ref())?;
    let mut share = KeyShare::from_slice(ciphertext.as_ref());
    share.extend_from_slice(x25519_share.as_ref());
    Ok((share, secret))
}

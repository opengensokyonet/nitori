use aws_lc_rs::aead;
use nitori_crypto::{
    Aead, AeadAlgorithm, AeadAuthenticationError, AeadInputTooLongError, AeadKey,
    AeadKeyLengthMismatchError, AeadOpenError, Supported,
};

use crate::{AwsLcBackend, token::Token};

fn aead_algorithm(algorithm: AeadAlgorithm) -> &'static aead::Algorithm {
    match algorithm {
        AeadAlgorithm::Aes128Gcm => &aead::AES_128_GCM,
        AeadAlgorithm::Aes256Gcm => &aead::AES_256_GCM,
        AeadAlgorithm::ChaCha20Poly1305 => &aead::CHACHA20_POLY1305,
    }
}

/// An AEAD key of aws-lc-rs.
pub struct AwsLcAeadKey {
    algorithm: AeadAlgorithm,
    key: aead::LessSafeKey,
}

impl core::fmt::Debug for AwsLcAeadKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AwsLcAeadKey")
            .field("algorithm", &self.algorithm)
            .finish_non_exhaustive()
    }
}

impl Aead for AwsLcBackend {
    type Key = AwsLcAeadKey;

    fn aead_algorithm(&self, algorithm: AeadAlgorithm) -> Option<Supported<Self, AeadAlgorithm>> {
        Some(Supported::new(Token::new(), algorithm))
    }

    fn aead_key(
        &self,
        algorithm: Supported<Self, AeadAlgorithm>,
        key: &[u8],
    ) -> Result<Self::Key, AeadKeyLengthMismatchError> {
        let algorithm = algorithm.algorithm();
        AeadKeyLengthMismatchError::check(algorithm, key)?;
        // invariant: aws-lc-rs refuses a key only when its length does not match
        // the algorithm's, which was checked above.
        let key =
            aead::UnboundKey::new(aead_algorithm(algorithm), key).expect("key length was checked");
        Ok(AwsLcAeadKey {
            algorithm,
            key: aead::LessSafeKey::new(key),
        })
    }
}

impl AeadKey for AwsLcAeadKey {
    fn seal_in_place(
        &self,
        nonce: &[u8; AeadAlgorithm::NONCE_LEN],
        aad: &[u8],
        in_out: &mut [u8],
    ) -> Result<[u8; AeadAlgorithm::TAG_LEN], AeadInputTooLongError> {
        AeadInputTooLongError::check(self.algorithm, in_out.len())?;
        // invariant: aws-lc-rs refuses to seal only an input beyond the algorithm's
        // limit, which was checked above.
        let tag = self
            .key
            .seal_in_place_separate_tag(
                aead::Nonce::assume_unique_for_key(*nonce),
                aead::Aad::from(aad),
                in_out,
            )
            .expect("input length was checked");
        // invariant: every aws-lc-rs AEAD tag is 16 bytes.
        Ok(tag
            .as_ref()
            .try_into()
            .expect("tag is AeadAlgorithm::TAG_LEN bytes"))
    }

    fn open_in_place(
        &self,
        nonce: &[u8; AeadAlgorithm::NONCE_LEN],
        aad: &[u8],
        in_out: &mut [u8],
        tag: &[u8; AeadAlgorithm::TAG_LEN],
    ) -> Result<(), AeadOpenError> {
        AeadInputTooLongError::check(self.algorithm, in_out.len())?;
        // Within the limit, aws-lc-rs refuses to open only when authentication
        // fails.
        self.key
            .open_in_place_separate_tag(
                aead::Nonce::assume_unique_for_key(*nonce),
                aead::Aad::from(aad),
                tag,
                in_out,
            )
            .map(|_| ())
            .map_err(|_| AeadAuthenticationError)?;
        Ok(())
    }
}

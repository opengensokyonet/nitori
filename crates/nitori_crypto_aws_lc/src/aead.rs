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

/// The longest text AWS-LC seals or opens with `algorithm` under one nonce.
///
/// AWS-LC refuses a ChaCha20-Poly1305 input of 2<sup>38</sup> − 64 bytes or
/// more (`crypto/cipher_extra/e_chacha20poly1305.c:164` for sealing, `:278`
/// for opening, in aws-lc-sys 0.47.0), one byte below the limit of RFC 8439
/// §2.8. For AES-GCM it refuses only a text longer than 2<sup>36</sup> − 32
/// bytes (`crypto/fipsmodule/modes/gcm.c:440`, `:522`, `:607` and `:709`),
/// the limit of the algorithm itself. aws-lc-rs adds no limit of its own.
const fn max_len(algorithm: AeadAlgorithm) -> u64 {
    match algorithm {
        AeadAlgorithm::Aes128Gcm | AeadAlgorithm::Aes256Gcm => {
            AeadInputTooLongError::max_len(algorithm)
        }
        AeadAlgorithm::ChaCha20Poly1305 => (1 << 38) - 65,
    }
}

/// Check that a text of `len` bytes is within the limit of AWS-LC.
fn check_len(algorithm: AeadAlgorithm, len: usize) -> Result<(), AeadInputTooLongError> {
    AeadInputTooLongError::check_within(algorithm, len, max_len(algorithm))
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
        check_len(self.algorithm, in_out.len())?;
        // invariant: aws-lc-rs refuses to seal only an input beyond the limit of
        // AWS-LC (see `max_len`), which was checked above.
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
        check_len(self.algorithm, in_out.len())?;
        // Within the limit of AWS-LC (see `max_len`), aws-lc-rs refuses to open
        // only when authentication fails.
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

#[cfg(test)]
mod tests {
    use nitori_crypto::{AeadAlgorithm, AeadInputTooLongError};

    use super::check_len;

    #[test]
    fn chacha20_poly1305_limit_is_one_byte_below_rfc() {
        let algorithm = AeadAlgorithm::ChaCha20Poly1305;
        let limit: u64 = (1 << 38) - 65;
        if let Ok(limit) = usize::try_from(limit) {
            assert!(check_len(algorithm, limit).is_ok());
            let error = check_len(algorithm, limit + 1).unwrap_err();
            assert_eq!(error.algorithm(), algorithm);
            assert_eq!(error.requested_len(), limit + 1);
            assert!(
                error
                    .to_string()
                    .contains(&format!("the {limit} byte limit"))
            );
        }
    }

    #[test]
    fn aes_gcm_limit_is_the_algorithm_limit() {
        for algorithm in [AeadAlgorithm::Aes128Gcm, AeadAlgorithm::Aes256Gcm] {
            let max = AeadInputTooLongError::max_len(algorithm);
            if let Ok(max) = usize::try_from(max) {
                assert!(check_len(algorithm, max).is_ok());
                let error = check_len(algorithm, max + 1).unwrap_err();
                assert_eq!(error.requested_len(), max + 1);
                assert!(error.to_string().contains(&format!("the {max} byte limit")));
            }
        }
    }
}

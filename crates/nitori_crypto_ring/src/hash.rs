use nitori_crypto::{
    Digest, ExpandLength, Hash, HashAlgorithm, Hkdf, Hmac, RunningHash, Secret, Supported,
    TagMismatchError,
};
use ring::{digest, hkdf, hmac};

use crate::{RingBackend, token::Token};

fn digest_algorithm(algorithm: HashAlgorithm) -> &'static digest::Algorithm {
    match algorithm {
        HashAlgorithm::Sha256 => &digest::SHA256,
        HashAlgorithm::Sha384 => &digest::SHA384,
    }
}

fn hmac_algorithm(algorithm: HashAlgorithm) -> hmac::Algorithm {
    match algorithm {
        HashAlgorithm::Sha256 => hmac::HMAC_SHA256,
        HashAlgorithm::Sha384 => hmac::HMAC_SHA384,
    }
}

fn hkdf_algorithm(algorithm: HashAlgorithm) -> hkdf::Algorithm {
    match algorithm {
        HashAlgorithm::Sha256 => hkdf::HKDF_SHA256,
        HashAlgorithm::Sha384 => hkdf::HKDF_SHA384,
    }
}

fn digest_of(algorithm: HashAlgorithm, bytes: &[u8]) -> Digest {
    // ring's output for `algorithm` has the algorithm's output length.
    Digest::new(algorithm, bytes).expect("ring output has the algorithm's length")
}

/// A ring hash computation; cloning it forks the computation.
#[derive(Clone)]
pub struct RingRunningHash {
    algorithm: HashAlgorithm,
    context: digest::Context,
}

impl RunningHash for RingRunningHash {
    fn update(&mut self, data: &[u8]) {
        self.context.update(data);
    }

    fn finish(self) -> Digest {
        digest_of(self.algorithm, self.context.finish().as_ref())
    }
}

impl Hash for RingBackend {
    type Running = RingRunningHash;

    fn hash_algorithm(&self, algorithm: HashAlgorithm) -> Option<Supported<Self, HashAlgorithm>> {
        Some(Supported::new(Token::new(), algorithm))
    }

    fn start(&self, algorithm: Supported<Self, HashAlgorithm>) -> Self::Running {
        let algorithm = algorithm.algorithm();
        RingRunningHash {
            algorithm,
            context: digest::Context::new(digest_algorithm(algorithm)),
        }
    }
}

impl Hmac for RingBackend {
    fn sign(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        key: &[u8],
        message: &[u8],
    ) -> Digest {
        let algorithm = algorithm.algorithm();
        let key = hmac::Key::new(hmac_algorithm(algorithm), key);
        digest_of(algorithm, hmac::sign(&key, message).as_ref())
    }

    fn verify(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        key: &[u8],
        message: &[u8],
        tag: &[u8],
    ) -> Result<(), TagMismatchError> {
        let key = hmac::Key::new(hmac_algorithm(algorithm.algorithm()), key);
        hmac::verify(&key, message, tag).map_err(|_| TagMismatchError)
    }
}

/// An HKDF output length for ring.
struct OutputLen(usize);

impl hkdf::KeyType for OutputLen {
    fn len(&self) -> usize {
        self.0
    }
}

impl Hkdf for RingBackend {
    fn extract(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        salt: &[u8],
        ikm: &[u8],
    ) -> Secret {
        // HKDF-Extract(salt, IKM) is HMAC-Hash(salt, IKM) (RFC 5869 §2.2).
        // ring's `Prk` does not expose its bytes, so HMAC computes it here.
        let key = hmac::Key::new(hmac_algorithm(algorithm.algorithm()), salt);
        Secret::new(hmac::sign(&key, ikm).as_ref()).expect("hash output fits in a secret")
    }

    fn expand(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        prk: &Secret,
        info: &[&[u8]],
        len: ExpandLength,
    ) -> Secret {
        let mut out = Secret::zeroed(len);
        let prk = hkdf::Prk::new_less_safe(hkdf_algorithm(algorithm.algorithm()), prk.as_bytes());
        // ring rejects only outputs above 255 hash lengths, and an
        // `ExpandLength` is at most 64 bytes.
        prk.expand(info, OutputLen(out.len()))
            .and_then(|okm| okm.fill(out.as_mut_bytes()))
            .expect("expand length is within 255 hash lengths");
        out
    }
}

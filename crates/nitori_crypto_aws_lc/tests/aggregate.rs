//! A backend that delegates each capability to another type.
//!
//! `TlsCrypto` stands for the set a protocol layer requires; the delegating
//! type implements each capability and never `TlsCrypto` itself.

use nitori_crypto::{
    BadSignatureError, Digest, ExpandLength, Hash, HashAlgorithm, Hkdf, Hmac, Issuer, KeyExchange,
    KeyGenerationFailedError, KeyShare, NamedGroup, Random, RandomError, RunningHash, Secret,
    ServerShareError, SignatureScheme, SignatureVerify, StartedExchange, Supported,
    TagMismatchError,
};
use nitori_crypto_aws_lc::AwsLcBackend;
use nitori_crypto_ring::RingBackend;

trait TlsCrypto: Hash + Hmac + Hkdf + KeyExchange + SignatureVerify + Random {}

impl<C: Hash + Hmac + Hkdf + KeyExchange + SignatureVerify + Random> TlsCrypto for C {}

/// Hashing, HMAC, HKDF and random bytes from `P`, key exchange from `K`,
/// signature verification from `V`.
#[derive(Clone, Copy, Default)]
struct Delegating<P, K, V> {
    primitives: P,
    key_exchange: K,
    verify: V,
}

mod token {
    pub struct Token(());

    impl Token {
        pub(crate) fn new() -> Self {
            Self(())
        }
    }
}

impl<P, K, V> Issuer for Delegating<P, K, V> {
    type Token = token::Token;
}

impl<P: Hash, K, V> Delegating<P, K, V> {
    /// The credential of `P` for an algorithm this type supports.
    fn hash_of(&self, algorithm: Supported<Self, HashAlgorithm>) -> Supported<P, HashAlgorithm> {
        // This type issues a credential only when `P` does, and `P`'s support
        // does not depend on the value.
        self.primitives
            .hash_algorithm(algorithm.algorithm())
            .expect("issued only for algorithms P supports")
    }
}

impl<P: Hash, K, V> Hash for Delegating<P, K, V> {
    type Running = P::Running;

    fn hash_algorithm(&self, algorithm: HashAlgorithm) -> Option<Supported<Self, HashAlgorithm>> {
        self.primitives
            .hash_algorithm(algorithm)
            .map(|_| Supported::new(token::Token::new(), algorithm))
    }

    fn start(&self, algorithm: Supported<Self, HashAlgorithm>) -> Self::Running {
        Hash::start(&self.primitives, self.hash_of(algorithm))
    }
}

impl<P: Hmac, K, V> Hmac for Delegating<P, K, V> {
    fn sign(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        key: &[u8],
        message: &[u8],
    ) -> Digest {
        Hmac::sign(&self.primitives, self.hash_of(algorithm), key, message)
    }

    fn verify(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        key: &[u8],
        message: &[u8],
        tag: &[u8],
    ) -> Result<(), TagMismatchError> {
        Hmac::verify(&self.primitives, self.hash_of(algorithm), key, message, tag)
    }
}

impl<P: Hkdf, K, V> Hkdf for Delegating<P, K, V> {
    fn extract(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        salt: &[u8],
        ikm: &[u8],
    ) -> Secret {
        self.primitives.extract(self.hash_of(algorithm), salt, ikm)
    }

    fn expand(
        &self,
        algorithm: Supported<Self, HashAlgorithm>,
        prk: &Secret,
        info: &[&[u8]],
        len: ExpandLength,
    ) -> Secret {
        self.primitives
            .expand(self.hash_of(algorithm), prk, info, len)
    }
}

impl<P: Random, K, V> Random for Delegating<P, K, V> {
    fn random<const N: usize>(&self) -> Result<[u8; N], RandomError> {
        self.primitives.random()
    }
}

impl<P, K: KeyExchange, V> Delegating<P, K, V> {
    fn group_of(&self, group: Supported<Self, NamedGroup>) -> Supported<K, NamedGroup> {
        // As for `hash_of`.
        self.key_exchange
            .named_group(group.algorithm())
            .expect("issued only for groups K supports")
    }
}

impl<P, K: KeyExchange, V> KeyExchange for Delegating<P, K, V> {
    type Started = K::Started;

    fn named_groups(&self) -> &[NamedGroup] {
        self.key_exchange.named_groups()
    }

    fn named_group(&self, group: NamedGroup) -> Option<Supported<Self, NamedGroup>> {
        self.key_exchange
            .named_group(group)
            .map(|_| Supported::new(token::Token::new(), group))
    }

    fn server_share(
        &self,
        group: Supported<Self, NamedGroup>,
        client_share: &[u8],
    ) -> Result<(KeyShare, Secret), ServerShareError> {
        self.key_exchange
            .server_share(self.group_of(group), client_share)
    }

    fn start(
        &self,
        group: Supported<Self, NamedGroup>,
    ) -> Result<Self::Started, KeyGenerationFailedError> {
        KeyExchange::start(&self.key_exchange, self.group_of(group))
    }
}

impl<P, K, V: SignatureVerify> SignatureVerify for Delegating<P, K, V> {
    fn signature_schemes(&self) -> &[SignatureScheme] {
        self.verify.signature_schemes()
    }

    fn signature_scheme(
        &self,
        scheme: SignatureScheme,
    ) -> Option<Supported<Self, SignatureScheme>> {
        self.verify
            .signature_scheme(scheme)
            .map(|_| Supported::new(token::Token::new(), scheme))
    }

    fn verify(
        &self,
        scheme: Supported<Self, SignatureScheme>,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), BadSignatureError> {
        // As for `hash_of`.
        let scheme = self
            .verify
            .signature_scheme(scheme.algorithm())
            .expect("issued only for schemes V supports");
        SignatureVerify::verify(&self.verify, scheme, public_key, message, signature)
    }
}

/// Uses every capability of `TlsCrypto` through the one bound.
fn handshake_like<C: TlsCrypto>(backend: &C, group: NamedGroup) -> Secret {
    let sha256 = backend.hash_algorithm(HashAlgorithm::Sha256).unwrap();
    let group = backend.named_group(group).unwrap();
    let random: [u8; 32] = backend.random().unwrap();

    let client = KeyExchange::start(backend, group).unwrap();
    let (server_share, server_secret) = backend
        .server_share(group, client.share().as_bytes())
        .unwrap();
    let client_secret = client.complete(server_share.as_bytes()).unwrap();
    assert_eq!(client_secret.as_bytes(), server_secret.as_bytes());

    let mut transcript = Hash::start(backend, sha256);
    transcript.update(&random);
    transcript.update(server_share.as_bytes());
    let digest = transcript.finish();
    let prk = backend.extract(sha256, &[], client_secret.as_bytes());
    let key = backend.expand(
        sha256,
        &prk,
        &[digest.as_bytes()],
        ExpandLength::hash_len(HashAlgorithm::Sha256),
    );
    let tag = Hmac::sign(backend, sha256, key.as_bytes(), digest.as_bytes());
    Hmac::verify(
        backend,
        sha256,
        key.as_bytes(),
        digest.as_bytes(),
        tag.as_bytes(),
    )
    .unwrap();

    let ed25519 = backend.signature_scheme(SignatureScheme::ED25519).unwrap();
    assert!(SignatureVerify::verify(backend, ed25519, &[0; 32], b"message", &[0; 64]).is_err());
    key
}

const _: () = {
    const fn tls<C: TlsCrypto>() {}
    tls::<Delegating<RingBackend, AwsLcBackend, RingBackend>>();
    tls::<Delegating<AwsLcBackend, RingBackend, AwsLcBackend>>();
};

#[test]
fn delegating_backend_satisfies_the_set() {
    let backend = Delegating {
        primitives: RingBackend,
        key_exchange: AwsLcBackend,
        verify: RingBackend,
    };
    // Key exchange comes from aws-lc-rs, so the hybrid group is available.
    assert_eq!(backend.named_groups(), AwsLcBackend.named_groups());
    assert!(backend.named_group(NamedGroup::X25519MLKEM768).is_some());
    // Verification comes from ring, so P-521 is not.
    assert!(
        backend
            .signature_scheme(SignatureScheme::ECDSA_SECP521R1_SHA512)
            .is_none()
    );
    handshake_like(&backend, NamedGroup::X25519MLKEM768);
    handshake_like(&backend, NamedGroup::SECP384R1);
}

#[test]
fn delegation_answers_like_the_delegate() {
    let backend = Delegating {
        primitives: AwsLcBackend,
        key_exchange: RingBackend,
        verify: AwsLcBackend,
    };
    assert!(backend.named_group(NamedGroup::X25519MLKEM768).is_none());
    assert!(
        backend
            .signature_scheme(SignatureScheme::ECDSA_SECP521R1_SHA512)
            .is_some()
    );
    handshake_like(&backend, NamedGroup::X25519);
}

/// The shape of a backend that reproduces RFC 8448: a fixed X25519 key with
/// aws-lc-rs for everything else.
#[cfg(feature = "fixed-x25519")]
#[test]
fn fixed_key_exchange_composes() {
    use nitori_crypto_aws_lc::FixedX25519;

    let backend = Delegating {
        primitives: AwsLcBackend,
        key_exchange: FixedX25519::new([7; 32]),
        verify: AwsLcBackend,
    };
    let first = handshake_like(&backend, NamedGroup::X25519);
    let second = handshake_like(&backend, NamedGroup::X25519);
    assert_ne!(first.as_bytes(), second.as_bytes());
    assert!(backend.named_group(NamedGroup::SECP256R1).is_none());
}

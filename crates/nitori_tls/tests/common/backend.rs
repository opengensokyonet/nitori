//! A backend that reproduces the traces of RFC 8448: aws-lc-rs for hashing,
//! HMAC, HKDF and random bytes, X25519 with a fixed private key, and a
//! signature check that accepts one expected signature over one expected
//! content.
//!
//! The server certificates of RFC 8448 have 1024-bit RSA keys, which
//! aws-lc-rs does not verify, so the check compares the content the chain
//! asks to verify with the content the test computes on its own.

use nitori_crypto::{
    BadSignatureError, Digest, ExpandLength, Hash, HashAlgorithm, Hkdf, Hmac, Issuer, KeyExchange,
    KeyGenerationFailedError, KeyShare, NamedGroup, Random, RandomError, Secret, ServerShareError,
    SignatureScheme, SignatureVerify, Supported, TagMismatchError,
};
use nitori_crypto_aws_lc::{AwsLcBackend, FixedX25519};
use std::rc::Rc;

/// The signature a [`TraceBackend`] accepts: of `content` under
/// `public_key`, with rsa_pss_rsae_sha256.
pub struct Expected {
    pub public_key: Vec<u8>,
    pub content: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Clone)]
pub struct TraceBackend {
    primitives: AwsLcBackend,
    key_exchange: FixedX25519,
    expected: Option<Rc<Expected>>,
}

impl TraceBackend {
    /// A backend whose X25519 key is `private_key`, accepting `expected`.
    pub fn new(private_key: [u8; 32], expected: Option<Expected>) -> Self {
        Self {
            primitives: AwsLcBackend,
            key_exchange: FixedX25519::new(private_key),
            expected: expected.map(Rc::new),
        }
    }

    /// The support for SHA-256.
    pub fn sha256(&self) -> Supported<Self, HashAlgorithm> {
        self.hash_algorithm(HashAlgorithm::Sha256).unwrap()
    }
}

mod token {
    pub struct Token(());

    impl Token {
        pub(crate) fn new() -> Self {
            Self(())
        }
    }
}

impl Issuer for TraceBackend {
    type Token = token::Token;
}

impl TraceBackend {
    fn hash_of(
        &self,
        hash: Supported<Self, HashAlgorithm>,
    ) -> Supported<AwsLcBackend, HashAlgorithm> {
        self.primitives.hash_algorithm(hash.algorithm()).unwrap()
    }
}

impl Hash for TraceBackend {
    type Running = <AwsLcBackend as Hash>::Running;

    fn hash_algorithm(&self, algorithm: HashAlgorithm) -> Option<Supported<Self, HashAlgorithm>> {
        self.primitives
            .hash_algorithm(algorithm)
            .map(|_| Supported::new(token::Token::new(), algorithm))
    }

    fn start(&self, algorithm: Supported<Self, HashAlgorithm>) -> Self::Running {
        Hash::start(&self.primitives, self.hash_of(algorithm))
    }
}

impl Hmac for TraceBackend {
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

impl Hkdf for TraceBackend {
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

impl Random for TraceBackend {
    fn random<const N: usize>(&self) -> Result<[u8; N], RandomError> {
        self.primitives.random()
    }
}

impl KeyExchange for TraceBackend {
    type Started = <FixedX25519 as KeyExchange>::Started;

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
        let group = self.key_exchange.named_group(group.algorithm()).unwrap();
        self.key_exchange.server_share(group, client_share)
    }

    fn start(
        &self,
        group: Supported<Self, NamedGroup>,
    ) -> Result<Self::Started, KeyGenerationFailedError> {
        let group = self.key_exchange.named_group(group.algorithm()).unwrap();
        KeyExchange::start(&self.key_exchange, group)
    }
}

const SCHEMES: &[SignatureScheme] = &[SignatureScheme::RSA_PSS_RSAE_SHA256];

impl SignatureVerify for TraceBackend {
    fn signature_schemes(&self) -> &[SignatureScheme] {
        SCHEMES
    }

    fn signature_scheme(
        &self,
        scheme: SignatureScheme,
    ) -> Option<Supported<Self, SignatureScheme>> {
        SCHEMES
            .contains(&scheme)
            .then(|| Supported::new(token::Token::new(), scheme))
    }

    fn verify(
        &self,
        _: Supported<Self, SignatureScheme>,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), BadSignatureError> {
        match &self.expected {
            Some(expected)
                if expected.public_key == public_key
                    && expected.content == message
                    && expected.signature == signature =>
            {
                Ok(())
            }
            _ => Err(BadSignatureError),
        }
    }
}

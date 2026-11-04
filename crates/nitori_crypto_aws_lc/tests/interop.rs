//! One backend starts, the other completes.

use core::{
    pin::pin,
    task::{Context, Poll, Waker},
};

use nitori_crypto::{
    ExpandLength, Hash, HashAlgorithm, Hkdf, Hmac, KeyExchange, NamedGroup, RunningHash, Signature,
    SignatureScheme, SignatureVerify, SigningKey, StartedExchange,
};
use nitori_crypto_aws_lc::AwsLcBackend;
use nitori_crypto_ring::RingBackend;
use ring::{rand::SystemRandom, signature};

const GROUPS: [NamedGroup; 3] = [
    NamedGroup::X25519,
    NamedGroup::SECP256R1,
    NamedGroup::SECP384R1,
];
const MESSAGE: &[u8] = b"signed content";

fn exchange<C: KeyExchange, S: KeyExchange>(client: &C, server: &S, group: NamedGroup) {
    let client_group = client.named_group(group).unwrap();
    let server_group = server.named_group(group).unwrap();
    let started = KeyExchange::start(client, client_group).unwrap();
    let (server_share, server_secret) = server
        .server_share(server_group, started.share().as_bytes())
        .unwrap();
    let client_secret = started.complete(server_share.as_bytes()).unwrap();
    assert_eq!(
        client_secret.as_bytes(),
        server_secret.as_bytes(),
        "{group:?}"
    );
}

#[test]
fn key_exchange_interoperates() {
    for group in GROUPS {
        exchange(&RingBackend, &AwsLcBackend, group);
        exchange(&AwsLcBackend, &RingBackend, group);
    }
}

#[test]
fn hash_hmac_and_hkdf_agree() {
    let ring = RingBackend;
    let aws_lc = AwsLcBackend;
    for algorithm in [HashAlgorithm::Sha256, HashAlgorithm::Sha384] {
        let r = ring.hash_algorithm(algorithm).unwrap();
        let a = aws_lc.hash_algorithm(algorithm).unwrap();

        let mut ring_hash = Hash::start(&ring, r);
        let mut aws_lc_hash = Hash::start(&aws_lc, a);
        ring_hash.update(b"transcript");
        aws_lc_hash.update(b"transcript");
        assert_eq!(ring_hash.finish(), aws_lc_hash.finish());

        assert_eq!(
            Hmac::sign(&ring, r, b"key", b"message"),
            Hmac::sign(&aws_lc, a, b"key", b"message")
        );

        let ring_prk = ring.extract(r, b"salt", b"ikm");
        let aws_lc_prk = aws_lc.extract(a, b"salt", b"ikm");
        assert_eq!(ring_prk.as_bytes(), aws_lc_prk.as_bytes());
        let len = ExpandLength::new(40).unwrap();
        assert_eq!(
            ring.expand(r, &ring_prk, &[b"in", b"fo"], len).as_bytes(),
            aws_lc.expand(a, &aws_lc_prk, &[b"info"], len).as_bytes()
        );
    }
}

fn sign<K: SigningKey>(key: &K, scheme: SignatureScheme) -> Signature {
    let mut signing = pin!(key.sign(scheme, MESSAGE));
    match signing
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(Ok(signature)) => signature,
        Poll::Ready(Err(_)) => panic!("signing with {scheme:?} failed"),
        Poll::Pending => panic!("local signing completes immediately"),
    }
}

fn verify<V: SignatureVerify>(
    backend: &V,
    scheme: SignatureScheme,
    public_key: &[u8],
    signature: &Signature,
) {
    let supported = backend.signature_scheme(scheme).unwrap();
    SignatureVerify::verify(
        backend,
        supported,
        public_key,
        MESSAGE,
        signature.as_bytes(),
    )
    .unwrap();
}

#[test]
fn signatures_interoperate() {
    let random = SystemRandom::new();
    for (algorithm, scheme) in [
        (
            &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            SignatureScheme::ECDSA_SECP256R1_SHA256,
        ),
        (
            &signature::ECDSA_P384_SHA384_ASN1_SIGNING,
            SignatureScheme::ECDSA_SECP384R1_SHA384,
        ),
    ] {
        let pkcs8 = signature::EcdsaKeyPair::generate_pkcs8(algorithm, &random).unwrap();
        let ring_key = nitori_crypto_ring::EcdsaSigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
        let aws_lc_key = nitori_crypto_aws_lc::EcdsaSigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
        assert_eq!(ring_key.public_key(), aws_lc_key.public_key());
        verify(
            &AwsLcBackend,
            scheme,
            ring_key.public_key(),
            &sign(&ring_key, scheme),
        );
        verify(
            &RingBackend,
            scheme,
            aws_lc_key.public_key(),
            &sign(&aws_lc_key, scheme),
        );
    }

    let pkcs8 = signature::Ed25519KeyPair::generate_pkcs8(&random).unwrap();
    let ring_key = nitori_crypto_ring::Ed25519SigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
    let aws_lc_key = nitori_crypto_aws_lc::Ed25519SigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
    let scheme = SignatureScheme::ED25519;
    // Ed25519 is deterministic.
    assert_eq!(sign(&ring_key, scheme), sign(&aws_lc_key, scheme));
    verify(
        &AwsLcBackend,
        scheme,
        ring_key.public_key(),
        &sign(&ring_key, scheme),
    );
    verify(
        &RingBackend,
        scheme,
        aws_lc_key.public_key(),
        &sign(&aws_lc_key, scheme),
    );

    let rsa = aws_lc_rs::signature::RsaKeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
    let pkcs8 =
        aws_lc_rs::encoding::AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&rsa).unwrap();
    let ring_key = nitori_crypto_ring::RsaSigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
    let aws_lc_key = nitori_crypto_aws_lc::RsaSigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
    assert_eq!(ring_key.public_key(), aws_lc_key.public_key());
    for scheme in [
        SignatureScheme::RSA_PSS_RSAE_SHA256,
        SignatureScheme::RSA_PSS_RSAE_SHA384,
        SignatureScheme::RSA_PSS_RSAE_SHA512,
    ] {
        verify(
            &AwsLcBackend,
            scheme,
            ring_key.public_key(),
            &sign(&ring_key, scheme),
        );
        verify(
            &RingBackend,
            scheme,
            aws_lc_key.public_key(),
            &sign(&aws_lc_key, scheme),
        );
    }
}

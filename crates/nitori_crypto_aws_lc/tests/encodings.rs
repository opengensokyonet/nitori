//! Both backends take only the encodings of shares and public keys that the
//! traits give, though aws-lc-rs itself parses more.

use core::{
    pin::pin,
    task::{Context, Poll, Waker},
};

use aws_lc_rs::{
    agreement,
    encoding::{AsBigEndian, AsDer, EcPublicKeyCompressedBin, PublicKeyX509Der},
    rsa::KeySize,
    signature::{self as aws_signature, KeyPair},
};
use nitori_crypto::{
    KeyExchange, NamedGroup, ServerShareError, Signature, SignatureScheme, SignatureVerify,
    SigningKey, StartedExchange,
};
use nitori_crypto_aws_lc::AwsLcBackend;
use nitori_crypto_ring::RingBackend;

const MESSAGE: &[u8] = b"signed content";

/// A public key of `algorithm` in encodings TLS does not allow: the
/// SubjectPublicKeyInfo and, on the NIST curves, the compressed and hybrid
/// points.
fn disallowed_shares(algorithm: &'static agreement::Algorithm) -> Vec<(&'static str, Vec<u8>)> {
    let key = agreement::PrivateKey::generate(algorithm).unwrap();
    let public = key.compute_public_key().unwrap();
    let spki = AsDer::<PublicKeyX509Der>::as_der(&public).unwrap();
    let mut shares = vec![("SubjectPublicKeyInfo", spki.as_ref().to_vec())];
    if *algorithm != agreement::X25519 {
        let compressed = AsBigEndian::<EcPublicKeyCompressedBin>::as_be_bytes(&public).unwrap();
        // A hybrid point is the uncompressed point with the parity of y in
        // the first byte (SEC 1 §2.3.3, ANSI X9.62).
        let mut hybrid = public.as_ref().to_vec();
        hybrid[0] = 0x06 | (hybrid.last().unwrap() & 1);
        shares.push(("compressed point", compressed.as_ref().to_vec()));
        shares.push(("hybrid point", hybrid));
    }
    shares
}

fn rejects_share<K: KeyExchange>(backend: &K, group: NamedGroup, share: &[u8], form: &str) {
    let supported = backend.named_group(group).unwrap();
    assert!(
        matches!(
            backend.server_share(supported, share),
            Err(ServerShareError::InvalidPeerShare { .. })
        ),
        "server takes a {form} for {group:?}"
    );
    let started = KeyExchange::start(backend, supported).unwrap();
    assert!(
        started.complete(share).is_err(),
        "client takes a {form} for {group:?}"
    );
}

#[test]
fn key_exchange_takes_only_tls_shares() {
    for (group, algorithm) in [
        (NamedGroup::X25519, &agreement::X25519),
        (NamedGroup::SECP256R1, &agreement::ECDH_P256),
        (NamedGroup::SECP384R1, &agreement::ECDH_P384),
    ] {
        for (form, share) in disallowed_shares(algorithm) {
            rejects_share(&AwsLcBackend, group, &share, form);
            rejects_share(&RingBackend, group, &share, form);
        }
    }
    // ring has no P-521.
    for (form, share) in disallowed_shares(&agreement::ECDH_P521) {
        rejects_share(&AwsLcBackend, NamedGroup::SECP521R1, &share, form);
    }
}

#[cfg(feature = "fixed-x25519")]
#[test]
fn fixed_x25519_takes_only_tls_shares() {
    let fixed = nitori_crypto_aws_lc::FixedX25519::new([7; 32]);
    for (form, share) in disallowed_shares(&agreement::X25519) {
        rejects_share(&fixed, NamedGroup::X25519, &share, form);
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

/// Whether `backend` verifies `signature` under `public_key`.
fn verifies<V: SignatureVerify>(
    backend: &V,
    scheme: SignatureScheme,
    public_key: &[u8],
    signature: &Signature,
) -> bool {
    let supported = backend.signature_scheme(scheme).unwrap();
    SignatureVerify::verify(
        backend,
        supported,
        public_key,
        MESSAGE,
        signature.as_bytes(),
    )
    .is_ok()
}

/// Check that each backend verifies `signature` under `public_key` and
/// under none of `disallowed`.
fn verifies_only_trait_form(
    scheme: SignatureScheme,
    public_key: &[u8],
    signature: &Signature,
    disallowed: &[(&str, Vec<u8>)],
    with_ring: bool,
) {
    assert!(verifies(&AwsLcBackend, scheme, public_key, signature));
    if with_ring {
        assert!(verifies(&RingBackend, scheme, public_key, signature));
    }
    for (form, key) in disallowed {
        assert!(
            !verifies(&AwsLcBackend, scheme, key, signature),
            "aws-lc takes a {form} for {scheme:?}"
        );
        if with_ring {
            assert!(
                !verifies(&RingBackend, scheme, key, signature),
                "ring takes a {form} for {scheme:?}"
            );
        }
    }
}

#[test]
fn verify_takes_only_the_bit_string_content() {
    let random = aws_lc_rs::rand::SystemRandom::new();
    for (algorithm, scheme, with_ring) in [
        (
            &aws_signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            SignatureScheme::ECDSA_SECP256R1_SHA256,
            true,
        ),
        (
            &aws_signature::ECDSA_P384_SHA384_ASN1_SIGNING,
            SignatureScheme::ECDSA_SECP384R1_SHA384,
            true,
        ),
        (
            &aws_signature::ECDSA_P521_SHA512_ASN1_SIGNING,
            SignatureScheme::ECDSA_SECP521R1_SHA512,
            false,
        ),
    ] {
        let pkcs8 = aws_signature::EcdsaKeyPair::generate_pkcs8(algorithm, &random).unwrap();
        let pair = aws_signature::EcdsaKeyPair::from_pkcs8(algorithm, pkcs8.as_ref()).unwrap();
        let key = nitori_crypto_aws_lc::EcdsaSigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = pair.public_key();
        let spki = AsDer::<PublicKeyX509Der>::as_der(public).unwrap();
        let compressed = AsBigEndian::<EcPublicKeyCompressedBin>::as_be_bytes(public).unwrap();
        let mut hybrid = public.as_ref().to_vec();
        hybrid[0] = 0x06 | (hybrid.last().unwrap() & 1);
        verifies_only_trait_form(
            scheme,
            key.public_key(),
            &sign(&key, scheme),
            &[
                ("SubjectPublicKeyInfo", spki.as_ref().to_vec()),
                ("compressed point", compressed.as_ref().to_vec()),
                ("hybrid point", hybrid),
            ],
            with_ring,
        );
    }

    let pkcs8 = aws_signature::Ed25519KeyPair::generate_pkcs8(&random).unwrap();
    let pair = aws_signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
    let key = nitori_crypto_aws_lc::Ed25519SigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
    let spki = AsDer::<PublicKeyX509Der>::as_der(pair.public_key()).unwrap();
    let scheme = SignatureScheme::ED25519;
    verifies_only_trait_form(
        scheme,
        key.public_key(),
        &sign(&key, scheme),
        &[("SubjectPublicKeyInfo", spki.as_ref().to_vec())],
        true,
    );

    let pair = aws_signature::RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
    let pkcs8 = AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&pair).unwrap();
    let key = nitori_crypto_aws_lc::RsaSigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
    let spki = AsDer::<PublicKeyX509Der>::as_der(pair.public_key()).unwrap();
    let pss = SignatureScheme::RSA_PSS_RSAE_SHA256;
    let disallowed = [("SubjectPublicKeyInfo", spki.as_ref().to_vec())];
    verifies_only_trait_form(pss, key.public_key(), &sign(&key, pss), &disallowed, true);
    // The signing key signs only the schemes of TLS 1.3, so sign with
    // PKCS #1 v1.5 directly.
    let mut pkcs1 = vec![0; pair.public_modulus_len()];
    pair.sign(
        &aws_signature::RSA_PKCS1_SHA256,
        &random,
        MESSAGE,
        &mut pkcs1,
    )
    .unwrap();
    verifies_only_trait_form(
        SignatureScheme::RSA_PKCS1_SHA256,
        key.public_key(),
        &Signature::from_slice(&pkcs1),
        &disallowed,
        true,
    );
}

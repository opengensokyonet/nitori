use core::{
    pin::pin,
    task::{Context, Poll, Waker},
};

use aws_lc_rs::{
    encoding::AsDer,
    rand::SystemRandom,
    rsa::KeySize,
    signature::{self, KeyPair},
};
use nitori_crypto::{Signature, SignatureScheme, SignatureVerify, SigningKey};
use nitori_crypto_aws_lc::{AwsLcBackend, EcdsaSigningKey, Ed25519SigningKey, RsaSigningKey};

const MESSAGE: &[u8] = b"signed content";

/// Sign `MESSAGE`, checking that the signature completes on its first poll.
fn sign<K: SigningKey>(key: &K, scheme: SignatureScheme) -> Result<Signature, K::Error> {
    let mut signing = pin!(key.sign(scheme, MESSAGE));
    match signing
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("local signing completes immediately"),
    }
}

fn assert_verifies(scheme: SignatureScheme, public_key: &[u8], signature: &[u8]) {
    let backend = AwsLcBackend;
    let supported = backend.signature_scheme(scheme).unwrap();
    SignatureVerify::verify(&backend, supported, public_key, MESSAGE, signature).unwrap();
    assert!(
        SignatureVerify::verify(&backend, supported, public_key, b"other content", signature)
            .is_err()
    );
    let mut tampered = signature.to_vec();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert!(SignatureVerify::verify(&backend, supported, public_key, MESSAGE, &tampered).is_err());
}

fn rsa_pkcs8() -> Vec<u8> {
    let key = signature::RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
    AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&key)
        .unwrap()
        .as_ref()
        .to_vec()
}

#[test]
fn ecdsa_signs_and_verifies() {
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
        (
            &signature::ECDSA_P521_SHA512_ASN1_SIGNING,
            SignatureScheme::ECDSA_SECP521R1_SHA512,
        ),
    ] {
        let pkcs8 = signature::EcdsaKeyPair::generate_pkcs8(algorithm, &random).unwrap();
        let key = EcdsaSigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
        assert_eq!(key.schemes(), [scheme]);
        let signature = sign(&key, scheme).unwrap();
        assert_verifies(scheme, key.public_key(), signature.as_bytes());
        assert!(sign(&key, SignatureScheme::ED25519).is_err());
    }
}

#[test]
fn ed25519_signs_and_verifies() {
    let pkcs8 = signature::Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let key = Ed25519SigningKey::from_pkcs8(pkcs8.as_ref()).unwrap();
    let signature = sign(&key, SignatureScheme::ED25519).unwrap();
    assert_eq!(signature.as_bytes().len(), 64);
    assert_verifies(
        SignatureScheme::ED25519,
        key.public_key(),
        signature.as_bytes(),
    );
    let error = sign(&key, SignatureScheme::ECDSA_SECP256R1_SHA256).unwrap_err();
    assert_eq!(error.scheme, SignatureScheme::ECDSA_SECP256R1_SHA256);
}

#[test]
fn rsa_pss_signs_and_verifies() {
    let key = RsaSigningKey::from_pkcs8(&rsa_pkcs8()).unwrap();
    for scheme in [
        SignatureScheme::RSA_PSS_RSAE_SHA256,
        SignatureScheme::RSA_PSS_RSAE_SHA384,
        SignatureScheme::RSA_PSS_RSAE_SHA512,
    ] {
        assert!(key.schemes().contains(&scheme));
        let signature = sign(&key, scheme).unwrap();
        assert_eq!(signature.as_bytes().len(), 256);
        assert_verifies(scheme, key.public_key(), signature.as_bytes());
    }
    // TLS 1.3 handshake signatures never use PKCS#1 v1.5.
    assert!(sign(&key, SignatureScheme::RSA_PKCS1_SHA256).is_err());
}

#[test]
fn rsa_pkcs1_verifies() {
    let key = signature::RsaKeyPair::from_pkcs8(&rsa_pkcs8()).unwrap();
    for (padding, scheme) in [
        (
            &signature::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA256,
        ),
        (
            &signature::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA384,
        ),
        (
            &signature::RSA_PKCS1_SHA512,
            SignatureScheme::RSA_PKCS1_SHA512,
        ),
    ] {
        let mut out = vec![0; key.public_modulus_len()];
        key.sign(padding, &SystemRandom::new(), MESSAGE, &mut out)
            .unwrap();
        assert_verifies(scheme, key.public_key().as_ref(), &out);
    }
}

#[test]
fn keys_of_other_algorithms_are_rejected() {
    let ed25519 = signature::Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    assert!(EcdsaSigningKey::from_pkcs8(ed25519.as_ref()).is_err());
    assert!(RsaSigningKey::from_pkcs8(ed25519.as_ref()).is_err());
    assert!(Ed25519SigningKey::from_pkcs8(&rsa_pkcs8()).is_err());
}

#[test]
fn listed_schemes_are_supported() {
    let backend = AwsLcBackend;
    for &scheme in backend.signature_schemes() {
        assert!(backend.signature_scheme(scheme).is_some(), "{scheme:?}");
    }
    // rsa_pkcs1_sha1 and an unassigned code point.
    assert!(
        backend
            .signature_scheme(SignatureScheme::new(0x0201))
            .is_none()
    );
    assert!(
        backend
            .signature_scheme(SignatureScheme::new(0x9999))
            .is_none()
    );
}

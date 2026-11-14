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
use nitori_crypto_aws_lc::{
    AwsLcBackend, EcdsaKeyDerError, EcdsaSigningKey, Ed25519KeyDerError, Ed25519SigningKey,
    RsaKeyDerError, RsaSigningKey,
};
use rustls_pki_types::{PrivateKeyDer, PrivateSec1KeyDer, pem::PemObject};

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

/// The private key in `pem`, PEM text as openssl writes it.
fn key_der(pem: &str) -> PrivateKeyDer<'static> {
    PrivateKeyDer::from_pem_slice(pem.as_bytes()).unwrap()
}

/// Made by `openssl ecparam -name prime256v1 -genkey`, as the QUIC interop
/// runner makes its server key: an EC PARAMETERS section, then the key in
/// SEC1.
const ECDSA_P256_SEC1: &str = include_str!("data/ecdsa-p256.sec1.pem");
/// The same key converted to PKCS#8 by `openssl pkcs8 -topk8 -nocrypt`.
const ECDSA_P256_PKCS8: &str = include_str!("data/ecdsa-p256.pk8.pem");
const ECDSA_P384_SEC1: &str = include_str!("data/ecdsa-p384.sec1.pem");
/// Made by `openssl genrsa -traditional 2048`.
const RSA_2048_PKCS1: &str = include_str!("data/rsa-2048.pkcs1.pem");
/// The same key converted to PKCS#8.
const RSA_2048_PKCS8: &str = include_str!("data/rsa-2048.pk8.pem");
const ED25519_PKCS8: &str = include_str!("data/ed25519.pk8.pem");

#[test]
fn ecdsa_keys_load_from_pkcs8_and_sec1() {
    let from_sec1 = EcdsaSigningKey::from_der(&key_der(ECDSA_P256_SEC1)).unwrap();
    let from_pkcs8 = EcdsaSigningKey::from_der(&key_der(ECDSA_P256_PKCS8)).unwrap();
    assert_eq!(from_sec1.public_key(), from_pkcs8.public_key());
    let scheme = SignatureScheme::ECDSA_SECP256R1_SHA256;
    assert_eq!(from_sec1.schemes(), [scheme]);
    assert_verifies(
        scheme,
        from_sec1.public_key(),
        sign(&from_sec1, scheme).unwrap().as_bytes(),
    );

    let p384 = EcdsaSigningKey::from_der(&key_der(ECDSA_P384_SEC1)).unwrap();
    let scheme = SignatureScheme::ECDSA_SECP384R1_SHA384;
    assert_eq!(p384.schemes(), [scheme]);
    assert_verifies(
        scheme,
        p384.public_key(),
        sign(&p384, scheme).unwrap().as_bytes(),
    );
}

#[test]
fn rsa_keys_load_from_pkcs8_and_pkcs1() {
    let from_pkcs1 = RsaSigningKey::from_der(&key_der(RSA_2048_PKCS1)).unwrap();
    let from_pkcs8 = RsaSigningKey::from_der(&key_der(RSA_2048_PKCS8)).unwrap();
    assert_eq!(from_pkcs1.public_key(), from_pkcs8.public_key());
    let scheme = SignatureScheme::RSA_PSS_RSAE_SHA256;
    assert_verifies(
        scheme,
        from_pkcs1.public_key(),
        sign(&from_pkcs1, scheme).unwrap().as_bytes(),
    );
}

#[test]
fn ed25519_keys_load_from_pkcs8() {
    let key = Ed25519SigningKey::from_der(&key_der(ED25519_PKCS8)).unwrap();
    let scheme = SignatureScheme::ED25519;
    assert_verifies(
        scheme,
        key.public_key(),
        sign(&key, scheme).unwrap().as_bytes(),
    );
}

#[test]
fn key_documents_of_another_format_or_algorithm_are_rejected() {
    let sec1 = key_der(ECDSA_P256_SEC1);
    let pkcs1 = key_der(RSA_2048_PKCS1);
    assert!(matches!(
        EcdsaSigningKey::from_der(&pkcs1),
        Err(EcdsaKeyDerError::UnsupportedFormat)
    ));
    assert!(matches!(
        RsaSigningKey::from_der(&sec1),
        Err(RsaKeyDerError::UnsupportedFormat)
    ));
    assert!(matches!(
        Ed25519SigningKey::from_der(&sec1),
        Err(Ed25519KeyDerError::UnsupportedFormat)
    ));
    assert!(matches!(
        Ed25519SigningKey::from_der(&pkcs1),
        Err(Ed25519KeyDerError::UnsupportedFormat)
    ));

    // A PKCS#8 document of another algorithm.
    let ed25519 = key_der(ED25519_PKCS8);
    assert!(matches!(
        EcdsaSigningKey::from_der(&ed25519),
        Err(EcdsaKeyDerError::Pkcs8 { .. })
    ));
    assert!(matches!(
        RsaSigningKey::from_der(&ed25519),
        Err(RsaKeyDerError::Pkcs8 { .. })
    ));
    assert!(matches!(
        Ed25519SigningKey::from_der(&key_der(ECDSA_P256_PKCS8)),
        Err(Ed25519KeyDerError::Pkcs8 { .. })
    ));

    // Documents that are not keys of their format.
    let not_sec1 = PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(pkcs1.secret_der()));
    assert!(matches!(
        EcdsaSigningKey::from_der(&not_sec1),
        Err(EcdsaKeyDerError::Sec1)
    ));
    let not_pkcs1 = PrivateKeyDer::Pkcs1(sec1.secret_der().into());
    assert!(matches!(
        RsaSigningKey::from_der(&not_pkcs1),
        Err(RsaKeyDerError::Pkcs1 { .. })
    ));
}

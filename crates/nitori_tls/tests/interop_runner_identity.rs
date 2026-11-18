//! A server identity made from the files the QUIC interop runner gives a
//! server: `/certs/cert.pem` and `/certs/priv.key`.
//!
//! `testdata/interop-runner` holds the output of the runner's `certs.sh`
//! with a chain of two: the leaf and one intermediate in `cert.pem`, the
//! leaf's P-256 key in `priv.key` as `openssl ecparam -genkey` writes it,
//! an EC PARAMETERS section followed by the key in SEC1.

use core::{
    pin::pin,
    task::{Context, Poll, Waker},
};

use nitori_crypto::{SignatureScheme, SignatureVerify};
use nitori_crypto_aws_lc::{AwsLcBackend, EcdsaSigningKey};
use nitori_tls::server::{
    identity::{CertificateSelection, CertifiedKey},
    targets::SelectedTarget,
};
use rustls_pki_types::{PrivateKeyDer, pem::PemObject};

const CERT_PEM: &[u8] = include_bytes!("../testdata/interop-runner/cert.pem");
const PRIV_KEY: &[u8] = include_bytes!("../testdata/interop-runner/priv.key");

#[test]
fn the_interop_runner_files_make_an_identity_that_signs_for_its_leaf() {
    let key = PrivateKeyDer::from_pem_slice(PRIV_KEY).unwrap();
    let key = EcdsaSigningKey::from_der(&key).unwrap();
    let identity = CertifiedKey::from_pem_chain(CERT_PEM, key).unwrap();

    let [leaf, intermediate] = identity.certificates() else {
        panic!("the chain is the leaf and one intermediate");
    };
    let contains = |haystack: &[u8], needle: &[u8]| {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    };
    assert!(contains(leaf, b"interop runner leaf"));
    assert!(contains(intermediate, b"interop runner intermediate 1"));
    // The leaf's SubjectPublicKeyInfo holds the key's public point.
    let public_key = identity.key().public_key();
    assert!(contains(leaf, public_key));

    let scheme = SignatureScheme::ECDSA_SECP256R1_SHA256;
    let selected = SelectedTarget {
        target: (),
        application_protocol: None,
    };
    let presented = identity
        .select(&selected, &[SignatureScheme::ED25519, scheme])
        .unwrap();
    assert_eq!(presented.scheme(), scheme);
    let content = b"certificate verify content";
    let Poll::Ready(signature) = pin!(presented.sign(content))
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("local signing completes immediately");
    };
    let backend = AwsLcBackend;
    let supported = backend.signature_scheme(scheme).unwrap();
    SignatureVerify::verify(
        &backend,
        supported,
        public_key,
        content,
        signature.unwrap().as_bytes(),
    )
    .unwrap();
}

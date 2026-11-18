//! Verification of a real chain with embedded SCTs.
//!
//! The fixed data in `testdata/docsrs`, all from public sources, fetched on
//! 2026-11-04:
//!
//! - `docsrs-com.der`: the certificate of docsrs.com, www.docs.rs and
//!   www.docsrs.com, valid from 2026-09-29T00:00:00Z to
//!   2027-04-14T23:59:59Z, with three embedded SCTs, from crt.sh
//!   (<https://crt.sh/?d=29628207099>).
//! - `amazon-rsa-2048-m04.der`: its issuer, Amazon RSA 2048 M04, from the
//!   leaf's CA Issuers address (<http://crt.r2m04.amazontrust.com/r2m04.cer>),
//!   SHA-256 138bdf6e23ac971eb4e626b279dd6a26f057510f1de394293a5eea2860de019b.
//! - `amazon-root-ca-1.der`: Amazon Root CA 1, from Amazon Trust Services'
//!   repository (<https://www.amazontrust.com/repository/AmazonRootCA1.cer>),
//!   SHA-256 8ecde6884f3d87b1125ba31ac3fcb13d7016de7f57cc904fe1cb97c6ae98196e.
//! - `*.spki.der`: the keys of the three logs that signed the SCTs, from
//!   Google's list of all logs
//!   (<https://www.gstatic.com/ct/log_list/v3/all_logs_list.json>, version
//!   96.0, published 2026-11-03T13:38:43Z): Google Xenon2027h1, Cloudflare
//!   Nimbus2027 and Let's Encrypt Sycamore2027h1, each Usable.

use super::{
    AnchorStore, CertificateVerifier, NoRevocationCheck, PathPosition, SpkiDistrustList,
    VerifyCertificateChainError, WebPkiVerifier,
    der::{self, Reader},
    public_key::PublicKeyAlgorithm,
    transparency::{
        CtLog, EmbeddedSctPolicy, LogId, LogList, LogState, StaleLogListAction, TransparencyError,
    },
};
use crate::{
    alert::AlertDescription, client::ServerTarget, clock::ManualClock,
    message::certificate::CertificateEntry,
};
use bytes::Bytes;
use core::convert::Infallible;
use nitori_crypto::{Hash, HashAlgorithm, RunningHash, SignatureScheme};
use nitori_crypto_aws_lc::AwsLcBackend;
use std::time::Duration;
use time::UtcDateTime;

const LEAF: &[u8] = include_bytes!("../../testdata/docsrs/docsrs-com.der");
const INTERMEDIATE: &[u8] = include_bytes!("../../testdata/docsrs/amazon-rsa-2048-m04.der");
const ROOT: &[u8] = include_bytes!("../../testdata/docsrs/amazon-root-ca-1.der");
const XENON: &[u8] = include_bytes!("../../testdata/docsrs/google-xenon2027h1.spki.der");
const NIMBUS: &[u8] = include_bytes!("../../testdata/docsrs/cloudflare-nimbus2027.spki.der");
const SYCAMORE: &[u8] = include_bytes!("../../testdata/docsrs/letsencrypt-sycamore2027h1.spki.der");

/// 2026-11-03T13:38:43Z, when the log list was published.
const PUBLISHED: u64 = 1_793_713_123;
/// 2026-11-04T00:00:00Z.
const NOW: u64 = 1_793_750_400;
/// 2027-04-14T23:59:59Z, the last second of the leaf's validity.
const NOT_AFTER: u64 = 1_807_747_199;

fn at(seconds: u64) -> UtcDateTime {
    UtcDateTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn log_id(key: &[u8]) -> LogId {
    let backend = AwsLcBackend;
    let mut hash = Hash::start(
        &backend,
        backend.hash_algorithm(HashAlgorithm::Sha256).unwrap(),
    );
    hash.update(key);
    LogId(hash.finish().as_bytes().try_into().unwrap())
}

fn log(key: &[u8], operator: &str, state: LogState) -> CtLog {
    CtLog::new(log_id(key), key, operator, state).unwrap()
}

fn logs() -> Vec<CtLog> {
    vec![
        log(XENON, "Google", LogState::Usable),
        log(NIMBUS, "Cloudflare", LogState::Usable),
        log(SYCAMORE, "Let's Encrypt", LogState::Usable),
    ]
}

fn chain() -> Vec<CertificateEntry<Bytes>> {
    [LEAF, INTERMEDIATE]
        .into_iter()
        .map(|der| CertificateEntry::new(Bytes::from_static(der), Vec::new()).unwrap())
        .collect()
}

/// The DER SubjectPublicKeyInfo of a certificate.
fn spki(certificate: &[u8]) -> &[u8] {
    let mut parts = Reader::new(Reader::single(certificate, der::SEQUENCE).unwrap());
    let mut fields = Reader::new(parts.expect(der::SEQUENCE).unwrap().contents);
    fields.optional(0xa0).unwrap();
    for _ in 0..5 {
        fields.read().unwrap();
    }
    fields.expect(der::SEQUENCE).unwrap().encoded
}

type Verifier = WebPkiVerifier<
    AwsLcBackend,
    AnchorStore,
    SpkiDistrustList,
    NoRevocationCheck,
    EmbeddedSctPolicy,
    ManualClock,
>;

fn verifier(policy: EmbeddedSctPolicy, now: UtcDateTime) -> Verifier {
    WebPkiVerifier::new(
        AwsLcBackend,
        AnchorStore::from_certificates([ROOT]).unwrap(),
        SpkiDistrustList::default(),
        NoRevocationCheck,
        policy,
        ManualClock::new(now),
    )
}

fn policy() -> EmbeddedSctPolicy {
    EmbeddedSctPolicy::new(LogList::new(logs(), at(PUBLISHED)))
}

fn target(name: &str) -> ServerTarget {
    ServerTarget::from_host_name(name).unwrap()
}

type Error = VerifyCertificateChainError<Infallible, TransparencyError>;

fn verify(verifier: &Verifier, name: &str) -> Result<super::SubjectPublicKey, Error> {
    verifier.verify_now(&target(name), &chain())
}

#[test]
fn the_chain_verifies_with_its_embedded_scts() {
    let verifier = verifier(policy(), at(NOW));
    let key = verify(&verifier, "docsrs.com").unwrap();
    assert_eq!(key.algorithm(), PublicKeyAlgorithm::Rsa);
    assert!(verify(&verifier, "www.docs.rs").is_ok());
    assert!(
        key.check_certificate_verify_scheme(SignatureScheme::RSA_PSS_RSAE_SHA256)
            .is_ok()
    );
}

#[test]
fn the_verifier_as_a_capability() {
    let verifier = verifier(policy(), at(NOW));
    let support = verifier.support();
    assert_eq!(
        support.signature_schemes()[0],
        SignatureScheme::ECDSA_SECP256R1_SHA256
    );
    assert!(
        support
            .signature_schemes()
            .contains(&SignatureScheme::RSA_PKCS1_SHA256)
    );
    assert!(!support.signed_certificate_timestamps());
    let target = target("docsrs.com");
    let chain = chain();
    let mut verifying = core::pin::pin!(verifier.verify(&target, &chain));
    let poll = verifying
        .as_mut()
        .poll(&mut core::task::Context::from_waker(
            core::task::Waker::noop(),
        ));
    assert!(matches!(poll, core::task::Poll::Ready(Ok(_))));
}

#[test]
fn another_name_fails() {
    let error = verify(&verifier(policy(), at(NOW)), "docs.rs").unwrap_err();
    assert!(
        matches!(error, VerifyCertificateChainError::Name { .. }),
        "{error:?}"
    );
    assert_eq!(error.alert(), AlertDescription::BAD_CERTIFICATE);
}

#[test]
fn an_expired_certificate_fails() {
    let error = verify(&verifier(policy(), at(NOT_AFTER + 1)), "docsrs.com").unwrap_err();
    assert!(
        matches!(
            error,
            VerifyCertificateChainError::Path {
                source: webpki::Error::CertExpired { .. }
            }
        ),
        "{error:?}"
    );
    assert_eq!(error.alert(), AlertDescription::CERTIFICATE_EXPIRED);
}

#[test]
fn a_distrusted_key_fails_wherever_it_is() {
    let verifier = verifier(policy(), at(NOW));
    verifier.distrust().insert(spki(INTERMEDIATE));
    let error = verify(&verifier, "docsrs.com").unwrap_err();
    assert!(
        matches!(
            error,
            VerifyCertificateChainError::Distrusted {
                position: PathPosition::Intermediate { depth: 1 }
            }
        ),
        "{error:?}"
    );
    verifier.distrust().replace([spki(ROOT)]);
    let error = verify(&verifier, "docsrs.com").unwrap_err();
    assert!(
        matches!(
            error,
            VerifyCertificateChainError::Distrusted {
                position: PathPosition::Anchor
            }
        ),
        "{error:?}"
    );
    verifier.distrust().replace([spki(LEAF)]);
    let error = verify(&verifier, "docsrs.com").unwrap_err();
    assert!(matches!(
        error,
        VerifyCertificateChainError::Distrusted {
            position: PathPosition::EndEntity
        }
    ));
    assert_eq!(error.alert(), AlertDescription::BAD_CERTIFICATE);
    verifier.distrust().replace([]);
    assert!(verify(&verifier, "docsrs.com").is_ok());
}

#[test]
fn an_unknown_root_fails() {
    let verifier = verifier(policy(), at(NOW));
    verifier.anchors().replace([]).unwrap();
    let error = verify(&verifier, "docsrs.com").unwrap_err();
    assert!(
        matches!(
            error,
            VerifyCertificateChainError::Path {
                source: webpki::Error::UnknownIssuer
            }
        ),
        "{error:?}"
    );
    assert_eq!(error.alert(), AlertDescription::UNKNOWN_CA);
    let error = verifier.verify_now(&target("docsrs.com"), &[]).unwrap_err();
    assert!(matches!(error, VerifyCertificateChainError::NoCertificate));
    assert_eq!(error.alert(), AlertDescription::DECODE_ERROR);
}

fn transparency_error(policy: EmbeddedSctPolicy, now: UtcDateTime) -> TransparencyError {
    match verify(&verifier(policy, now), "docsrs.com").unwrap_err() {
        VerifyCertificateChainError::Transparency { source } => source,
        error => panic!("{error:?}"),
    }
}

#[test]
fn a_long_validity_needs_three_scts() {
    // The leaf is valid for 198 days, above 180, so it needs 3 SCTs.
    let mut two = logs();
    two.pop();
    let error = transparency_error(
        EmbeddedSctPolicy::new(LogList::new(two, at(PUBLISHED))),
        at(NOW),
    );
    assert!(
        matches!(
            error,
            TransparencyError::NotEnoughTimestamps {
                valid: 2,
                required: 3
            }
        ),
        "{error:?}"
    );
}

#[test]
fn an_sct_from_a_log_retired_before_it_does_not_count() {
    let mut logs = logs();
    // The SCTs were issued on 2026-09-29.
    logs[0] = log(
        XENON,
        "Google",
        LogState::Retired {
            at: at(1_790_640_000),
        },
    );
    let error = transparency_error(
        EmbeddedSctPolicy::new(LogList::new(logs.clone(), at(PUBLISHED))),
        at(NOW),
    );
    assert!(matches!(
        error,
        TransparencyError::NotEnoughTimestamps { valid: 2, .. }
    ));
    logs[0] = log(XENON, "Google", LogState::Retired { at: at(NOW) });
    assert!(
        verify(
            &verifier(
                EmbeddedSctPolicy::new(LogList::new(logs, at(PUBLISHED))),
                at(NOW)
            ),
            "docsrs.com"
        )
        .is_ok()
    );
}

#[test]
fn the_scts_come_from_two_operators_at_least() {
    let one_operator = vec![
        log(XENON, "Google", LogState::Usable),
        log(NIMBUS, "Google", LogState::Usable),
        log(SYCAMORE, "Google", LogState::Usable),
    ];
    let error = transparency_error(
        EmbeddedSctPolicy::new(LogList::new(one_operator, at(PUBLISHED))),
        at(NOW),
    );
    assert!(
        matches!(error, TransparencyError::TooFewOperators),
        "{error:?}"
    );
}

#[test]
fn a_stale_log_list_fails_closed_by_default() {
    let late = at(PUBLISHED) + EmbeddedSctPolicy::CHROME_MAX_LIST_AGE + Duration::from_secs(1);
    let error = transparency_error(policy(), late);
    assert!(
        matches!(error, TransparencyError::StaleLogList { .. }),
        "{error:?}"
    );
    let open = EmbeddedSctPolicy::with_list_age(
        LogList::new(Vec::new(), at(PUBLISHED)),
        EmbeddedSctPolicy::CHROME_MAX_LIST_AGE,
        StaleLogListAction::SkipCheck,
    );
    assert!(verify(&verifier(open, late), "docsrs.com").is_ok());
    // A fresh list replaces the stale one.
    let verifier = verifier(policy(), late);
    assert!(verify(&verifier, "docsrs.com").is_err());
    verifier
        .transparency()
        .replace_list(LogList::new(logs(), late));
    assert!(verify(&verifier, "docsrs.com").is_ok());
}

#[test]
fn a_log_key_must_be_p256_or_rsa() {
    let mut ed25519 = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    ed25519.extend_from_slice(&[7; 32]);
    assert!(CtLog::new(LogId([0; 32]), &ed25519, "x", LogState::Usable).is_err());
}

#[test]
fn the_ids_of_the_logs_are_those_of_the_list() {
    // From the `log_id` fields of the list.
    assert_eq!(
        log_id(XENON).0[..4],
        [0x44, 0xc2, 0xbd, 0x0c],
        "Xenon2027h1"
    );
    assert_eq!(
        log_id(NIMBUS).0[..4],
        [0x4c, 0x63, 0xdc, 0x98],
        "Nimbus2027"
    );
    assert_eq!(
        log_id(SYCAMORE).0[..4],
        [0x8e, 0xca, 0x47, 0x0b],
        "Sycamore2027h1"
    );
}

#[test]
fn an_sct_counts_only_with_its_signature() {
    use super::Transparency as _;
    let policy = policy();
    let issuer = spki(INTERMEDIATE);
    assert!(policy.check(&AwsLcBackend, LEAF, issuer, at(NOW)).is_ok());
    // The start of Nimbus2027's signature in the leaf.
    let signature = [0x30, 0x45, 0x02, 0x21, 0x00, 0x95, 0xb1, 0x6c];
    let position = LEAF
        .windows(signature.len())
        .position(|window| window == signature)
        .unwrap();
    let mut tampered = LEAF.to_vec();
    tampered[position + 6] ^= 1;
    let error = policy
        .check(&AwsLcBackend, &tampered, issuer, at(NOW))
        .unwrap_err();
    assert!(matches!(
        error,
        TransparencyError::NotEnoughTimestamps { valid: 2, .. }
    ));
    // Signed for another issuer, no SCT counts.
    let error = policy
        .check(&AwsLcBackend, LEAF, spki(ROOT), at(NOW))
        .unwrap_err();
    assert!(matches!(
        error,
        TransparencyError::NotEnoughTimestamps { valid: 0, .. }
    ));
    // An SCT from the future does not count.
    let error = policy
        .check(&AwsLcBackend, LEAF, issuer, at(1_790_640_000))
        .unwrap_err();
    assert!(matches!(
        error,
        TransparencyError::NotEnoughTimestamps { valid: 0, .. }
    ));
    let error = policy
        .check(&AwsLcBackend, &LEAF[..LEAF.len() - 1], issuer, at(NOW))
        .unwrap_err();
    assert!(matches!(
        error,
        TransparencyError::MalformedCertificate { .. }
    ));
}

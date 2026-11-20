//! An Ed25519 root and a certificate it issues for example.com, in DER,
//! built here and signed with nitori_crypto_aws_lc; and the client's
//! verifier that trusts the root alone.

use super::ready;
use nitori_crypto::{SignatureScheme, SigningKey};
use nitori_crypto_aws_lc::{AwsLcBackend, Ed25519SigningKey};
use nitori_sync::MutexSynchronization;
use nitori_tls::{
    clock::ManualClock,
    verify::{
        AnchorStore, NoRevocationCheck, SpkiDistrustList, SubjectPublicKey, WebPkiVerifier,
        transparency::{EmbeddedSctPolicy, LogList, StaleLogListAction},
    },
};
use std::{sync::Arc, time::Duration};
use time::UtcDateTime;

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    match content.len() {
        len @ 0..=127 => out.push(len as u8),
        len @ 128..=255 => out.extend([0x81, len as u8]),
        len => out.extend([0x82, (len >> 8) as u8, len as u8]),
    }
    out.extend_from_slice(content);
    out
}

fn seq(parts: &[&[u8]]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

fn name(common_name: &str) -> Vec<u8> {
    let attribute = seq(&[
        &tlv(0x06, &[0x55, 0x04, 0x03]),
        &tlv(0x0c, common_name.as_bytes()),
    ]);
    seq(&[&tlv(0x31, &attribute)])
}

/// id-Ed25519 as an AlgorithmIdentifier.
fn ed25519() -> Vec<u8> {
    seq(&[&tlv(0x06, &[0x2b, 0x65, 0x70])])
}

fn bits(bytes: &[u8]) -> Vec<u8> {
    tlv(0x03, &[&[0], bytes].concat())
}

pub fn spki(public_key: &[u8]) -> Vec<u8> {
    seq(&[&ed25519(), &bits(public_key)])
}

fn extension(oid: &[u8], critical: bool, value: &[u8]) -> Vec<u8> {
    let critical = if critical {
        tlv(0x01, &[0xff])
    } else {
        Vec::new()
    };
    seq(&[&tlv(0x06, oid), &critical, &tlv(0x04, value)])
}

/// The PKCS#8 v1 document of the Ed25519 key with the 32-byte seed
/// `[seed; 32]`.
pub fn pkcs8(seed: u8) -> Vec<u8> {
    let mut pkcs8 = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    pkcs8.extend([seed; 32]);
    pkcs8
}

/// The key of `pkcs8(seed)`.
pub fn key(seed: u8) -> Ed25519SigningKey {
    Ed25519SigningKey::from_pkcs8(&pkcs8(seed)).unwrap()
}

fn certificate(
    serial: u8,
    issuer: &str,
    subject: &str,
    public_key: &[u8],
    extensions: &[Vec<u8>],
    signer: &Ed25519SigningKey,
) -> Vec<u8> {
    let extensions: Vec<&[u8]> = extensions.iter().map(Vec::as_slice).collect();
    let validity = seq(&[&tlv(0x17, b"250101000000Z"), &tlv(0x17, b"491231235959Z")]);
    let tbs = seq(&[
        &tlv(0xa0, &tlv(0x02, &[2])),
        &tlv(0x02, &[serial]),
        &ed25519(),
        &name(issuer),
        &validity,
        &name(subject),
        &spki(public_key),
        &tlv(0xa3, &seq(&extensions)),
    ]);
    let signature = ready(signer.sign(SignatureScheme::ED25519, &tbs)).unwrap();
    seq(&[&tbs, &ed25519(), &bits(signature.as_bytes())])
}

/// The root, self-signed by `key(1)`.
pub fn root() -> Vec<u8> {
    let root = key(1);
    let basic_constraints = extension(&[0x55, 0x1d, 0x13], true, &seq(&[&tlv(0x01, &[0xff])]));
    certificate(
        1,
        "Test Root",
        "Test Root",
        root.public_key(),
        &[basic_constraints],
        &root,
    )
}

/// The certificate of `key(2)` for example.com, issued by the root.
pub fn leaf() -> Vec<u8> {
    let alternative_names = extension(
        &[0x55, 0x1d, 0x11],
        false,
        &seq(&[&tlv(0x82, b"example.com")]),
    );
    let server_auth = extension(
        &[0x55, 0x1d, 0x25],
        false,
        &seq(&[&tlv(
            0x06,
            &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01],
        )]),
    );
    certificate(
        2,
        "Test Root",
        "example.com",
        key(2).public_key(),
        &[alternative_names, server_auth],
        &key(1),
    )
}

pub type Verifier = WebPkiVerifier<
    AwsLcBackend,
    AnchorStore<MutexSynchronization>,
    SpkiDistrustList<MutexSynchronization>,
    NoRevocationCheck,
    EmbeddedSctPolicy<MutexSynchronization>,
    Arc<ManualClock<MutexSynchronization>>,
>;

pub fn verifier(clock: &Arc<ManualClock<MutexSynchronization>>) -> Verifier {
    // The certificates carry no SCTs: a log list past its age skips the CT
    // check.
    let transparency = EmbeddedSctPolicy::with_list_age(
        LogList::new(Vec::new(), UtcDateTime::UNIX_EPOCH),
        Duration::ZERO,
        StaleLogListAction::SkipCheck,
    );
    WebPkiVerifier::new(
        AwsLcBackend,
        AnchorStore::from_certificates([root().as_slice()]).unwrap(),
        SpkiDistrustList::default(),
        NoRevocationCheck,
        transparency,
        Arc::clone(clock),
    )
}

pub fn leaf_key() -> SubjectPublicKey {
    SubjectPublicKey::from_spki(&spki(key(2).public_key())).unwrap()
}

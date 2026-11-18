//! The transcript and key schedule against the values of RFC 8448 §3, §4
//! and §5, for both roles: the traffic secrets of every level, the Finished
//! messages, the exporter and resumption master secrets through what they
//! derive, ticket PSKs, the PSK binder and the message_hash that replaces
//! the first ClientHello after a HelloRetryRequest.
#![feature(impl_trait_in_assoc_type, yield_expr)]

mod common;

use common::{
    backend::{Expected, TraceBackend},
    hex,
    rfc8448::*,
};
use nitori_crypto::{
    ExpandLength, Hash as _, HashAlgorithm, Hkdf as _, KeyExchange, NamedGroup, RunningHash as _,
    Secret, StartedExchange as _,
};
use nitori_crypto_aws_lc::AwsLcBackend;
use nitori_tls::schedule::TrafficSecret;
use nitori_tls::{
    alert::AlertDescription,
    extension::pre_shared_key::PskIndex,
    message::{
        HandshakeMessage, MessageLimits, codec::ReadHandshakeMessageHostExt as _,
        new_session_ticket::TicketNonce,
    },
    role::{Client, Server},
    schedule::{
        Authentication, CertificateVerifyError, Exporter, ExporterLabel, FinishedError,
        ResumptionPsk, Transcript,
    },
};

/// The whole handshake message in `text`.
fn message(text: &str) -> HandshakeMessage {
    hex(text)
        .as_slice()
        .sync_read_handshake_message_unpin(MessageLimits::default(), Some(HashAlgorithm::Sha256))
        .unwrap()
}

fn key(text: &str) -> [u8; 32] {
    hex(text).try_into().unwrap()
}

fn secret(text: &str) -> Secret {
    Secret::new(&hex(text)).unwrap()
}

fn assert_secret(secret: &TrafficSecret, expected: &str) {
    assert_eq!(secret.as_bytes(), hex(expected));
}

fn sha256(parts: &[&[u8]]) -> Vec<u8> {
    let backend = AwsLcBackend;
    let mut hash = nitori_crypto::Hash::start(
        &backend,
        backend.hash_algorithm(HashAlgorithm::Sha256).unwrap(),
    );
    for part in parts {
        hash.update(part);
    }
    hash.finish().as_bytes().to_vec()
}

/// HKDF-Expand-Label with SHA-256, written out here apart from the crate.
fn expand_label(secret: &[u8], label: &[u8], context: &[u8], len: u8) -> Vec<u8> {
    let backend = AwsLcBackend;
    let sha256 = backend.hash_algorithm(HashAlgorithm::Sha256).unwrap();
    let mut info = vec![0, len, 6 + label.len() as u8];
    info.extend_from_slice(b"tls13 ");
    info.extend_from_slice(label);
    info.push(context.len() as u8);
    info.extend_from_slice(context);
    let prk = Secret::new(secret).unwrap();
    backend
        .expand(sha256, &prk, &[&info], ExpandLength::new(len).unwrap())
        .as_bytes()
        .to_vec()
}

/// Checks that `exporter` holds the exporter_master_secret `expected`, by
/// what it exports.
fn assert_exporter(exporter: &Exporter<TraceBackend>, expected: &str) {
    let label = b"EXPORTER-nitori test";
    let derived = expand_label(&hex(expected), label, &sha256(&[]), 32);
    let value = expand_label(&derived, b"exporter", &sha256(&[b"context"]), 32);
    let exported = exporter.export(
        &ExporterLabel::new(label).unwrap(),
        b"context",
        ExpandLength::new(32).unwrap(),
    );
    assert_eq!(exported.as_bytes(), value);
}

/// The ticket PSK of nonce 00 00 from the resumption_master_secret
/// `expected`.
fn ticket_psk(expected: &str) -> Vec<u8> {
    expand_label(&hex(expected), b"resumption", &[0, 0], 32)
}

/// The content of a server's CertificateVerify over `transcript`.
fn certificate_verify_content(transcript: &[&[u8]]) -> Vec<u8> {
    let mut content = vec![0x20; 64];
    content.extend_from_slice(b"TLS 1.3, server CertificateVerify\0");
    content.extend_from_slice(&sha256(transcript));
    content
}

/// The signature of a CertificateVerify message.
fn signature(certificate_verify: &str) -> Vec<u8> {
    hex(certificate_verify)[8..].to_vec()
}

const PUBLIC_KEY: &[u8] = b"the leaf key";

/// The nonce of the ticket of §3.
fn nonce() -> TicketNonce<&'static [u8]> {
    TicketNonce::new(&[0, 0][..]).unwrap()
}

#[test]
fn a_traffic_secret_is_as_long_as_its_hash() {
    let error = TrafficSecret::new(HashAlgorithm::Sha384, &[0; 32]).unwrap_err();
    assert_eq!((error.expected, error.actual), (48, 32));
    let mut secret = TrafficSecret::zeroed(HashAlgorithm::Sha384);
    secret.as_mut_bytes().fill(0x5a);
    assert_eq!(secret.hash(), HashAlgorithm::Sha384);
    assert_eq!(secret.as_bytes(), [0x5a; 48]);
    assert_eq!(format!("{secret:?}"), "TrafficSecret { hash: Sha384, .. }");
}

#[test]
fn labels_and_nonces_are_bounded() {
    assert_eq!(ExporterLabel::new(&[][..]).unwrap_err().length(), 0);
    assert!(ExporterLabel::new(&[0; 1][..]).is_ok());
    assert!(ExporterLabel::new(&[0; 249][..]).is_ok());
    assert_eq!(ExporterLabel::new(&[0; 250][..]).unwrap_err().length(), 250);
    assert!(TicketNonce::new(&[0; 255][..]).is_ok());
    assert_eq!(TicketNonce::new(&[0; 256][..]).unwrap_err().length(), 256);
}

#[test]
fn s3_server() {
    let backend = TraceBackend::new(key(s3::SERVER_PRIVATE), None);
    let group = backend.named_group(NamedGroup::X25519).unwrap();
    let (share, shared) = backend
        .server_share(group, &hex(s3::CLIENT_PUBLIC))
        .unwrap();
    assert_eq!(share.as_bytes(), hex(s3::SERVER_PUBLIC));
    assert_eq!(shared.as_bytes(), hex(s3::SHARED_SECRET));

    let (handshake, keys) = Transcript::<Server, _>::new(&backend, backend.sha256())
        .add(&hex(s3::CLIENT_HELLO))
        .early_secret()
        .server_hello(shared, &hex(s3::SERVER_HELLO));
    assert_secret(&keys.send, s3::SERVER_HANDSHAKE_TRAFFIC);
    assert_secret(&keys.receive, s3::CLIENT_HANDSHAKE_TRAFFIC);
    assert!(!handshake.resumed());

    let handshake = handshake
        .add(&hex(s3::ENCRYPTED_EXTENSIONS))
        .add(&hex(s3::CERTIFICATE));
    let content = certificate_verify_content(&[
        &hex(s3::CLIENT_HELLO),
        &hex(s3::SERVER_HELLO),
        &hex(s3::ENCRYPTED_EXTENSIONS),
        &hex(s3::CERTIFICATE),
    ]);
    assert_eq!(handshake.certificate_verify_content().as_bytes(), content);
    let (master, finished, one_rtt) = handshake
        .add(&hex(s3::CERTIFICATE_VERIFY))
        .server_finished();
    assert_eq!(finished.encoded().as_ref(), hex(s3::SERVER_FINISHED));
    assert_secret(&one_rtt.0, s3::SERVER_APPLICATION_TRAFFIC);
    assert_exporter(&master.exporter(), s3::EXPORTER_MASTER);

    let (resumption, completed) = master
        .client_finished(&message(s3::CLIENT_FINISHED))
        .unwrap();
    assert_secret(&completed.one_rtt_receive.0, s3::CLIENT_APPLICATION_TRAFFIC);
    assert_exporter(&completed.exporter, s3::EXPORTER_MASTER);
    let psk = resumption.ticket_psk(&nonce());
    assert_eq!(psk.secret().as_bytes(), hex(s3::TICKET_PSK));
    assert_eq!(ticket_psk(s3::RESUMPTION_MASTER), hex(s3::TICKET_PSK));
}

/// The server's chain of §3 up to the client's Finished.
fn s3_server_master() -> nitori_tls::schedule::MainSecret<Server, TraceBackend> {
    let backend = TraceBackend::new(key(s3::SERVER_PRIVATE), None);
    Transcript::<Server, _>::new(&backend, backend.sha256())
        .add(&hex(s3::CLIENT_HELLO))
        .early_secret()
        .server_hello(secret(s3::SHARED_SECRET), &hex(s3::SERVER_HELLO))
        .0
        .add(&hex(s3::ENCRYPTED_EXTENSIONS))
        .add(&hex(s3::CERTIFICATE))
        .add(&hex(s3::CERTIFICATE_VERIFY))
        .server_finished()
        .0
}

#[test]
fn s3_server_rejects_another_finished() {
    let mut tampered = hex(s3::CLIENT_FINISHED);
    tampered[10] ^= 1;
    let tampered = tampered
        .as_slice()
        .sync_read_handshake_message_unpin(MessageLimits::default(), Some(HashAlgorithm::Sha256))
        .unwrap();
    let Err(error) = s3_server_master().client_finished(&tampered) else {
        panic!()
    };
    assert!(matches!(error, FinishedError::Mismatch));
    assert_eq!(error.alert(), AlertDescription::DECRYPT_ERROR);
    // A message of another type is not a Finished.
    let Err(error) = s3_server_master().client_finished(&message(s3::ENCRYPTED_EXTENSIONS)) else {
        panic!()
    };
    assert_eq!(error.alert(), AlertDescription::UNEXPECTED_MESSAGE);
}

fn s3_client_backend(content: Vec<u8>) -> TraceBackend {
    TraceBackend::new(
        key(s3::CLIENT_PRIVATE),
        Some(Expected {
            public_key: PUBLIC_KEY.to_vec(),
            content,
            signature: signature(s3::CERTIFICATE_VERIFY),
        }),
    )
}

fn s3_content() -> Vec<u8> {
    certificate_verify_content(&[
        &hex(s3::CLIENT_HELLO),
        &hex(s3::SERVER_HELLO),
        &hex(s3::ENCRYPTED_EXTENSIONS),
        &hex(s3::CERTIFICATE),
    ])
}

#[test]
fn s3_client() {
    let backend = s3_client_backend(s3_content());
    let group = backend.named_group(NamedGroup::X25519).unwrap();
    let exchange = KeyExchange::start(&backend, group).unwrap();
    assert_eq!(exchange.share().as_bytes(), hex(s3::CLIENT_PUBLIC));
    let shared = exchange.complete(&hex(s3::SERVER_PUBLIC)).unwrap();

    let (handshake, keys) = Transcript::<Client, _>::new(&backend, backend.sha256())
        .add(&hex(s3::CLIENT_HELLO))
        .early_secret()
        .server_hello(shared, &hex(s3::SERVER_HELLO), None)
        .unwrap();
    assert_secret(&keys.send, s3::CLIENT_HANDSHAKE_TRAFFIC);
    assert_secret(&keys.receive, s3::SERVER_HANDSHAKE_TRAFFIC);
    let Authentication::Certificate(certificate) = handshake
        .add(&hex(s3::ENCRYPTED_EXTENSIONS))
        .authentication()
    else {
        panic!()
    };
    let authenticated = certificate
        .add(&hex(s3::CERTIFICATE))
        .verify_certificate_verify(&message(s3::CERTIFICATE_VERIFY), PUBLIC_KEY)
        .unwrap();
    let (master, keys) = authenticated
        .server_finished(&message(s3::SERVER_FINISHED))
        .unwrap();
    assert_secret(&keys.send, s3::CLIENT_APPLICATION_TRAFFIC);
    assert_secret(&keys.receive, s3::SERVER_APPLICATION_TRAFFIC);
    assert_exporter(&master.exporter(), s3::EXPORTER_MASTER);
    let (resumption, finished) = master.client_finished();
    assert_eq!(finished.encoded().as_ref(), hex(s3::CLIENT_FINISHED));
    let psk = resumption.ticket_psk(&nonce());
    assert_eq!(psk.secret().as_bytes(), hex(s3::TICKET_PSK));
}

/// The client's certificate branch of §3, after EncryptedExtensions.
fn s3_certificate_branch(
    content: Vec<u8>,
) -> nitori_tls::schedule::AwaitingCertificateAuthentication<TraceBackend> {
    let backend = s3_client_backend(content);
    let (handshake, _) = Transcript::<Client, _>::new(&backend, backend.sha256())
        .add(&hex(s3::CLIENT_HELLO))
        .early_secret()
        .server_hello(secret(s3::SHARED_SECRET), &hex(s3::SERVER_HELLO), None)
        .unwrap();
    let Authentication::Certificate(certificate) = handshake
        .add(&hex(s3::ENCRYPTED_EXTENSIONS))
        .authentication()
    else {
        panic!()
    };
    certificate
}

#[test]
fn s3_client_checks_the_signature() {
    // Without the Certificate in the transcript, the signature covers other
    // content.
    let Err(error) = s3_certificate_branch(s3_content())
        .verify_certificate_verify(&message(s3::CERTIFICATE_VERIFY), PUBLIC_KEY)
    else {
        panic!()
    };
    assert!(matches!(error, CertificateVerifyError::Signature { .. }));
    assert_eq!(error.alert(), AlertDescription::DECRYPT_ERROR);
    let Err(error) = s3_certificate_branch(s3_content())
        .add(&hex(s3::CERTIFICATE))
        .verify_certificate_verify(&message(s3::CERTIFICATE_VERIFY), b"another key")
    else {
        panic!()
    };
    assert!(matches!(error, CertificateVerifyError::Signature { .. }));
    // A scheme the backend does not verify.
    let mut ed25519 = hex(s3::CERTIFICATE_VERIFY);
    ed25519[4..6].copy_from_slice(&[0x08, 0x07]);
    let ed25519 = ed25519
        .as_slice()
        .sync_read_handshake_message_unpin(MessageLimits::default(), None)
        .unwrap();
    let Err(error) = s3_certificate_branch(s3_content())
        .add(&hex(s3::CERTIFICATE))
        .verify_certificate_verify(&ed25519, PUBLIC_KEY)
    else {
        panic!()
    };
    assert!(matches!(
        error,
        CertificateVerifyError::UnsupportedScheme { .. }
    ));
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
}

/// The bytes of the §4 ClientHello after the part the binder covers.
fn s4_binders() -> Vec<u8> {
    let hello = hex(s4::CLIENT_HELLO);
    hello[hex(s4::CLIENT_HELLO_PREFIX).len()..].to_vec()
}

fn s4_psk() -> ResumptionPsk {
    ResumptionPsk::new(secret(s4::PSK))
}

#[test]
fn s4_server() {
    let backend = TraceBackend::new(key(s4::SERVER_PRIVATE), None);
    let early = Transcript::<Server, _>::new(&backend, backend.sha256())
        .add(&hex(s4::CLIENT_HELLO_PREFIX))
        .early_secret_with_psk(s4_psk());
    early.verify_binder(&hex(s4::BINDER)).unwrap();
    let mut tampered = hex(s4::BINDER);
    tampered[0] ^= 1;
    let error = early.verify_binder(&tampered).unwrap_err();
    assert_eq!(error.alert(), AlertDescription::DECRYPT_ERROR);
    // The binder was computed on a fork: the transcript goes on with the
    // binders.
    let early = early.add(&s4_binders());
    assert_secret(&early.early_receive().0, s4::CLIENT_EARLY_TRAFFIC);

    let group = backend.named_group(NamedGroup::X25519).unwrap();
    let (share, shared) = backend
        .server_share(group, &hex(s4::CLIENT_PUBLIC))
        .unwrap();
    assert_eq!(share.as_bytes(), hex(s4::SERVER_PUBLIC));
    let (handshake, keys) = early.server_hello(shared, &hex(s4::SERVER_HELLO));
    assert!(handshake.resumed());
    assert_secret(&keys.send, s4::SERVER_HANDSHAKE_TRAFFIC);
    assert_secret(&keys.receive, s4::CLIENT_HANDSHAKE_TRAFFIC);
    let (master, finished, one_rtt) = handshake
        .add(&hex(s4::ENCRYPTED_EXTENSIONS))
        .server_finished();
    assert_eq!(finished.encoded().as_ref(), hex(s4::SERVER_FINISHED));
    assert_secret(&one_rtt.0, s4::SERVER_APPLICATION_TRAFFIC);
    // The client of §4 sends EndOfEarlyData, which QUIC does not; it is in
    // the transcript of the trace.
    let (resumption, completed) = master
        .add(&hex(s4::END_OF_EARLY_DATA))
        .client_finished(&message(s4::CLIENT_FINISHED))
        .unwrap();
    assert_secret(&completed.one_rtt_receive.0, s4::CLIENT_APPLICATION_TRAFFIC);
    assert_exporter(&completed.exporter, s4::EXPORTER_MASTER);
    let psk = resumption.ticket_psk(&nonce());
    assert_eq!(psk.secret().as_bytes(), ticket_psk(s4::RESUMPTION_MASTER));
}

#[test]
fn s4_client() {
    let backend = TraceBackend::new(key(s4::CLIENT_PRIVATE), None);
    let early = Transcript::<Client, _>::new(&backend, backend.sha256())
        .add(&hex(s4::CLIENT_HELLO_PREFIX))
        .early_secret_with_psk(s4_psk());
    assert_eq!(early.binder().as_bytes(), hex(s4::BINDER));
    let early = early.add(&s4_binders());
    assert_secret(&early.early_send().0, s4::CLIENT_EARLY_TRAFFIC);

    let group = backend.named_group(NamedGroup::X25519).unwrap();
    let exchange = KeyExchange::start(&backend, group).unwrap();
    assert_eq!(exchange.share().as_bytes(), hex(s4::CLIENT_PUBLIC));
    let shared = exchange.complete(&hex(s4::SERVER_PUBLIC)).unwrap();
    let (handshake, keys) = early
        .server_hello(shared, &hex(s4::SERVER_HELLO), Some(PskIndex::new(0)))
        .unwrap();
    assert_secret(&keys.send, s4::CLIENT_HANDSHAKE_TRAFFIC);
    let Authentication::Resumed(authenticated) = handshake
        .add(&hex(s4::ENCRYPTED_EXTENSIONS))
        .authentication()
    else {
        panic!()
    };
    let (master, keys) = authenticated
        .server_finished(&message(s4::SERVER_FINISHED))
        .unwrap();
    assert_secret(&keys.send, s4::CLIENT_APPLICATION_TRAFFIC);
    assert_secret(&keys.receive, s4::SERVER_APPLICATION_TRAFFIC);
    let (resumption, finished) = master.add(&hex(s4::END_OF_EARLY_DATA)).client_finished();
    assert_eq!(finished.encoded().as_ref(), hex(s4::CLIENT_FINISHED));
    let psk = resumption.ticket_psk(&nonce());
    assert_eq!(psk.secret().as_bytes(), ticket_psk(s4::RESUMPTION_MASTER));
}

#[test]
fn a_client_accepts_only_the_psk_it_offered() {
    let backend = TraceBackend::new(key(s4::CLIENT_PRIVATE), None);
    let transcript =
        Transcript::<Client, _>::new(&backend, backend.sha256()).add(&hex(s4::CLIENT_HELLO));
    let Err(error) = transcript
        .clone()
        .early_secret_with_psk(s4_psk())
        .server_hello(
            secret(s4::SHARED_SECRET),
            &hex(s4::SERVER_HELLO),
            Some(PskIndex::new(1)),
        )
    else {
        panic!()
    };
    assert_eq!(error.index(), PskIndex::new(1));
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
    let Err(error) = transcript.early_secret().server_hello(
        secret(s4::SHARED_SECRET),
        &hex(s4::SERVER_HELLO),
        Some(PskIndex::new(0)),
    ) else {
        panic!()
    };
    assert_eq!(error.index(), PskIndex::new(0));
}

#[test]
fn a_declined_psk_leaves_the_early_secret_without_it() {
    // A client that offered the PSK of §4 to the server of §3, which
    // declines it, derives the Handshake Secret as §3 does.
    let backend = TraceBackend::new(key(s3::CLIENT_PRIVATE), None);
    let (handshake, keys) = Transcript::<Client, _>::new(&backend, backend.sha256())
        .add(&hex(s3::CLIENT_HELLO))
        .early_secret_with_psk(s4_psk())
        .server_hello(secret(s3::SHARED_SECRET), &hex(s3::SERVER_HELLO), None)
        .unwrap();
    assert!(!handshake.resumed());
    assert_secret(&keys.send, s3::CLIENT_HANDSHAKE_TRAFFIC);
}

/// The message_hash message that stands for the first ClientHello of §5.
fn s5_message_hash() -> Vec<u8> {
    let mut message = vec![254, 0, 0, 32];
    message.extend_from_slice(&sha256(&[&hex(s5::CLIENT_HELLO_1)]));
    message
}

#[test]
fn s5_server() {
    let backend = TraceBackend::new([0; 32], None);
    let (handshake, keys) = Transcript::<Server, _>::new(&backend, backend.sha256())
        .add(&hex(s5::CLIENT_HELLO_1))
        .retry(&hex(s5::HELLO_RETRY_REQUEST))
        .add(&hex(s5::CLIENT_HELLO_2))
        .early_secret()
        // The second exchange is over P-256; the trace gives its secret.
        .server_hello(secret(s5::SHARED_SECRET), &hex(s5::SERVER_HELLO));
    assert_secret(&keys.send, s5::SERVER_HANDSHAKE_TRAFFIC);
    assert_secret(&keys.receive, s5::CLIENT_HANDSHAKE_TRAFFIC);
    let handshake = handshake
        .add(&hex(s5::ENCRYPTED_EXTENSIONS))
        .add(&hex(s5::CERTIFICATE));
    let content = certificate_verify_content(&[
        &s5_message_hash(),
        &hex(s5::HELLO_RETRY_REQUEST),
        &hex(s5::CLIENT_HELLO_2),
        &hex(s5::SERVER_HELLO),
        &hex(s5::ENCRYPTED_EXTENSIONS),
        &hex(s5::CERTIFICATE),
    ]);
    assert_eq!(handshake.certificate_verify_content().as_bytes(), content);
    let (master, finished, one_rtt) = handshake
        .add(&hex(s5::CERTIFICATE_VERIFY))
        .server_finished();
    assert_eq!(finished.encoded().as_ref(), hex(s5::SERVER_FINISHED));
    assert_secret(&one_rtt.0, s5::SERVER_APPLICATION_TRAFFIC);
    let (resumption, completed) = master
        .client_finished(&message(s5::CLIENT_FINISHED))
        .unwrap();
    assert_secret(&completed.one_rtt_receive.0, s5::CLIENT_APPLICATION_TRAFFIC);
    assert_exporter(&completed.exporter, s5::EXPORTER_MASTER);
    let psk = resumption.ticket_psk(&nonce());
    assert_eq!(psk.secret().as_bytes(), ticket_psk(s5::RESUMPTION_MASTER));
}

#[test]
fn s5_client() {
    let content = certificate_verify_content(&[
        &s5_message_hash(),
        &hex(s5::HELLO_RETRY_REQUEST),
        &hex(s5::CLIENT_HELLO_2),
        &hex(s5::SERVER_HELLO),
        &hex(s5::ENCRYPTED_EXTENSIONS),
        &hex(s5::CERTIFICATE),
    ]);
    let backend = TraceBackend::new(
        [0; 32],
        Some(Expected {
            public_key: PUBLIC_KEY.to_vec(),
            content,
            signature: signature(s5::CERTIFICATE_VERIFY),
        }),
    );
    let (handshake, keys) = Transcript::<Client, _>::new(&backend, backend.sha256())
        .add(&hex(s5::CLIENT_HELLO_1))
        .retry(&hex(s5::HELLO_RETRY_REQUEST))
        .add(&hex(s5::CLIENT_HELLO_2))
        .early_secret()
        .server_hello(secret(s5::SHARED_SECRET), &hex(s5::SERVER_HELLO), None)
        .unwrap();
    assert_secret(&keys.send, s5::CLIENT_HANDSHAKE_TRAFFIC);
    let Authentication::Certificate(certificate) = handshake
        .add(&hex(s5::ENCRYPTED_EXTENSIONS))
        .authentication()
    else {
        panic!()
    };
    let (master, keys) = certificate
        .add(&hex(s5::CERTIFICATE))
        .verify_certificate_verify(&message(s5::CERTIFICATE_VERIFY), PUBLIC_KEY)
        .unwrap()
        .server_finished(&message(s5::SERVER_FINISHED))
        .unwrap();
    assert_secret(&keys.send, s5::CLIENT_APPLICATION_TRAFFIC);
    assert_secret(&keys.receive, s5::SERVER_APPLICATION_TRAFFIC);
    assert_exporter(&master.exporter(), s5::EXPORTER_MASTER);
    let (_, finished) = master.client_finished();
    assert_eq!(finished.encoded().as_ref(), hex(s5::CLIENT_FINISHED));
}

use super::{
    HandshakeType, MessageLimit, MessageLimits, ReceivedMessageType,
    codec::{ReadHandshakeMessageError, ReadHandshakeMessageHostExt as _},
};
use crate::{alert::AlertDescription, test_support::message};
use bytes::BufMut;
use core::future::Future as _;
use core::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use nitori_crypto::HashAlgorithm;
use nitori_io::Read;

/// A source that waits before each byte.
struct Trickle {
    bytes: Vec<u8>,
    ready: bool,
}

nitori_call::family_receiver!(impl [] for Trickle);

impl Read for Trickle {
    type Error = Infallible;

    fn poll_read<'visit, Output: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let source = host.get_pin_mut().get_mut();
        if !source.ready {
            source.ready = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        source.ready = false;
        if source.bytes.is_empty() || !destination.has_remaining_mut() {
            return Poll::Ready(Ok(0));
        }
        destination.put_u8(source.bytes.remove(0));
        Poll::Ready(Ok(1))
    }
}

#[test]
fn a_message_is_collected_across_waits() {
    let encoded = message(8, &[0x00, 0x02, 0xaa, 0xbb]);
    let mut source = Trickle {
        bytes: [encoded.as_slice(), &[0x14]].concat(),
        ready: false,
    };
    let mut read =
        core::pin::pin!(source.read_handshake_message_unpin(MessageLimits::default(), None));
    let mut cx = Context::from_waker(Waker::noop());
    let mut pending = 0;
    let message = loop {
        match read.as_mut().poll(&mut cx) {
            Poll::Ready(message) => break message.unwrap(),
            Poll::Pending => pending += 1,
        }
    };
    assert!(pending >= encoded.len());
    assert_eq!(
        message.handshake_type(),
        HandshakeType::ENCRYPTED_EXTENSIONS
    );
    assert_eq!(message.encoded().as_ref(), encoded);
    // The next message has not been read.
    assert_eq!(source.bytes, [0x14]);
}

#[test]
fn a_length_above_the_limit_fails_before_the_body() {
    let limits = MessageLimits {
        certificate_verify: MessageLimit::new(4).unwrap(),
        ..MessageLimits::default()
    };
    let mut input = &[15, 0, 0, 5, 1, 2, 3, 4, 5][..];
    let error = input
        .sync_read_handshake_message_unpin(limits, None)
        .unwrap_err();
    let ReadHandshakeMessageError::TooLong { source } = &error else {
        panic!("{error:?}")
    };
    assert_eq!(
        (source.message_type(), source.length(), source.limit()),
        (ReceivedMessageType::CertificateVerify, 5, 4)
    );
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
    assert_eq!(input, [1, 2, 3, 4, 5]);
}

#[test]
fn finished_is_limited_by_the_hash() {
    let finished = message(20, &[7; 32]);
    let mut input = finished.as_slice();
    let error = input
        .sync_read_handshake_message_unpin(MessageLimits::default(), None)
        .unwrap_err();
    assert!(
        matches!(error, ReadHandshakeMessageError::Unexpected { source } if source.message_type() == HandshakeType::FINISHED)
    );
    assert_eq!(error.alert(), AlertDescription::UNEXPECTED_MESSAGE);
    let mut input = finished.as_slice();
    let read = input
        .sync_read_handshake_message_unpin(MessageLimits::default(), Some(HashAlgorithm::Sha256));
    assert_eq!(read.unwrap().encoded().as_ref(), finished);
    let longer = message(20, &[7; 33]);
    let error = longer
        .as_slice()
        .sync_read_handshake_message_unpin(MessageLimits::default(), Some(HashAlgorithm::Sha256))
        .unwrap_err();
    assert!(matches!(error, ReadHandshakeMessageError::TooLong { .. }));
}

#[test]
fn types_quic_does_not_carry_are_unexpected() {
    for message_type in [5, 24, 254, 3] {
        let encoded = message(message_type, &[0]);
        let error = encoded
            .as_slice()
            .sync_read_handshake_message_unpin(
                MessageLimits::default(),
                Some(HashAlgorithm::Sha256),
            )
            .unwrap_err();
        assert!(
            matches!(error, ReadHandshakeMessageError::Unexpected { .. }),
            "{message_type}"
        );
    }
}

#[test]
fn a_stream_that_ends_within_a_message_is_a_decode_error() {
    for input in [&[1, 0, 0][..], &[1, 0, 0, 4, 1, 2]] {
        let error = { input }
            .sync_read_handshake_message_unpin(MessageLimits::default(), None)
            .unwrap_err();
        assert_eq!(error.alert(), AlertDescription::DECODE_ERROR, "{error:?}");
    }
}

#[test]
fn limits_default_and_are_bounded() {
    let limits = MessageLimits::default();
    assert_eq!(
        limits.limit(ReceivedMessageType::ClientHello, None),
        Some(16 * 1024)
    );
    assert_eq!(
        limits.limit(ReceivedMessageType::Certificate, None),
        Some(64 * 1024)
    );
    assert_eq!(
        limits.limit(ReceivedMessageType::CertificateVerify, None),
        Some(8 * 1024)
    );
    assert_eq!(
        limits.limit(ReceivedMessageType::Finished, Some(HashAlgorithm::Sha384)),
        Some(48)
    );
    assert_eq!(MessageLimit::new(0xff_ffff).unwrap(), MessageLimit::MAX);
    assert_eq!(
        MessageLimit::new(0x100_0000).unwrap_err().length(),
        0x100_0000
    );
}

/// Each message's enum of disallowed extensions holds exactly the
/// recognized extensions RFC 9846 §4.3, RFC 9001 §8.2 and RFC 9849 do not
/// list for that message.
#[test]
fn extension_subset_follow_the_table() {
    use super::{
        certificate::CertificateEntryDisallowedExtension,
        certificate_request::CertificateRequestDisallowedExtension,
        encrypted_extensions::EncryptedExtensionsDisallowedExtension,
        new_session_ticket::NewSessionTicketDisallowedExtension,
        server_hello::{HelloRetryRequestDisallowedExtension, ServerHelloDisallowedExtension},
    };
    use crate::extension::RestrictedExtension::{self, *};

    fn check<X: Copy + core::fmt::Debug>(
        of: fn(RestrictedExtension) -> Option<X>,
        restricted: fn(X) -> RestrictedExtension,
        allowed: &[RestrictedExtension],
    ) {
        for extension in RestrictedExtension::ALL {
            match of(extension) {
                Some(disallowed) => {
                    assert!(!allowed.contains(&extension), "{extension:?}");
                    assert_eq!(restricted(disallowed), extension);
                }
                None => assert!(allowed.contains(&extension), "{extension:?}"),
            }
        }
    }

    check(
        ServerHelloDisallowedExtension::of,
        ServerHelloDisallowedExtension::restricted,
        &[KeyShare, PreSharedKey, SupportedVersions],
    );
    check(
        HelloRetryRequestDisallowedExtension::of,
        HelloRetryRequestDisallowedExtension::restricted,
        &[KeyShare, Cookie, SupportedVersions, EncryptedClientHello],
    );
    check(
        EncryptedExtensionsDisallowedExtension::of,
        EncryptedExtensionsDisallowedExtension::restricted,
        &[
            ServerName,
            SupportedGroups,
            ApplicationLayerProtocolNegotiation,
            ClientCertificateType,
            ServerCertificateType,
            EarlyData,
            QuicTransportParameters,
            EncryptedClientHello,
        ],
    );
    check(
        CertificateRequestDisallowedExtension::of,
        CertificateRequestDisallowedExtension::restricted,
        &[
            SignatureAlgorithms,
            SignatureAlgorithmsCert,
            SignedCertificateTimestamp,
        ],
    );
    check(
        CertificateEntryDisallowedExtension::of,
        CertificateEntryDisallowedExtension::restricted,
        &[SignedCertificateTimestamp],
    );
    check(
        NewSessionTicketDisallowedExtension::of,
        NewSessionTicketDisallowedExtension::restricted,
        &[EarlyData],
    );
}

/// Each message's enum of extensions whose data has a structure holds
/// exactly the extensions the message may carry, but for those whose data
/// is kept whole.
#[test]
fn structured_extensions_are_the_allowed_ones_with_a_structure() {
    use super::{
        certificate_request::CertificateRequestStructuredExtension,
        client_hello::ClientHelloStructuredExtension,
        encrypted_extensions::EncryptedExtensionsStructuredExtension,
        server_hello::{HelloRetryRequestStructuredExtension, ServerHelloStructuredExtension},
    };
    use crate::extension::RestrictedExtension::{self, *};

    fn check<X: Copy + core::fmt::Debug>(
        of: fn(RestrictedExtension) -> Option<X>,
        restricted: fn(X) -> RestrictedExtension,
        structured: &[RestrictedExtension],
    ) {
        for extension in RestrictedExtension::ALL {
            match of(extension) {
                Some(value) => {
                    assert!(structured.contains(&extension), "{extension:?}");
                    assert_eq!(restricted(value), extension);
                }
                None => assert!(!structured.contains(&extension), "{extension:?}"),
            }
        }
    }

    let opaque = [QuicTransportParameters, EncryptedClientHello];
    let every: Vec<_> = RestrictedExtension::ALL
        .into_iter()
        .filter(|extension| !opaque.contains(extension))
        .collect();
    check(
        ClientHelloStructuredExtension::of,
        ClientHelloStructuredExtension::restricted,
        &every,
    );
    check(
        ServerHelloStructuredExtension::of,
        ServerHelloStructuredExtension::restricted,
        &[KeyShare, PreSharedKey, SupportedVersions],
    );
    check(
        HelloRetryRequestStructuredExtension::of,
        HelloRetryRequestStructuredExtension::restricted,
        &[KeyShare, Cookie, SupportedVersions],
    );
    check(
        EncryptedExtensionsStructuredExtension::of,
        EncryptedExtensionsStructuredExtension::restricted,
        &[
            ServerName,
            SupportedGroups,
            ApplicationLayerProtocolNegotiation,
            ClientCertificateType,
            ServerCertificateType,
            EarlyData,
        ],
    );
    check(
        CertificateRequestStructuredExtension::of,
        CertificateRequestStructuredExtension::restricted,
        &[
            SignatureAlgorithms,
            SignatureAlgorithmsCert,
            SignedCertificateTimestamp,
        ],
    );
}

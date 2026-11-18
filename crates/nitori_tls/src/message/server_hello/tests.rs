use super::{
    HELLO_RETRY_REQUEST_RANDOM, InvalidServerHelloError, ServerHello, ServerHelloMessage,
    codec::{ReadServerHelloMessageError, read_server_hello_message},
};
use crate::{
    alert::AlertDescription,
    cipher_suite::CipherSuite,
    message::LegacySessionId,
    test_support::{concat, extension, message, vector},
    version::ProtocolVersion,
};

fn server_hello(random: &[u8; 32], compression: u8, extensions: &[u8]) -> Vec<u8> {
    message(
        2,
        &concat(&[
            &[0x03, 0x03],
            random,
            &[0],
            &[0x13, 0x01],
            &[compression],
            &vector(2, extensions),
        ]),
    )
}

fn read(bytes: Vec<u8>) -> Result<ServerHelloMessage<bytes::Bytes>, ReadServerHelloMessageError> {
    read_server_hello_message(&mut bytes::Bytes::from(bytes))
}

const VERSION: &[u8] = &[0x00, 0x2b, 0x00, 0x02, 0x03, 0x04];

#[test]
fn the_random_tells_a_retry_apart() {
    let hello = read(server_hello(&[7; 32], 0, VERSION)).unwrap();
    assert!(matches!(hello, ServerHelloMessage::ServerHello(_)));
    let retry = read(server_hello(
        &HELLO_RETRY_REQUEST_RANDOM.0,
        0,
        &concat(&[VERSION, &extension(51, &[0x00, 0x17])]),
    ))
    .unwrap();
    let ServerHelloMessage::HelloRetryRequest(retry) = retry else {
        panic!()
    };
    assert_eq!(
        retry.key_share(),
        Some(nitori_crypto::NamedGroup::SECP256R1)
    );
}

#[test]
fn each_message_allows_its_extensions() {
    // A ServerHello may not carry a cookie; a HelloRetryRequest may.
    let cookie = extension(44, &[0x00, 0x01, 0x09]);
    let error = read(server_hello(&[7; 32], 0, &concat(&[VERSION, &cookie]))).unwrap_err();
    let ReadServerHelloMessageError::ExtensionNotAllowed { source } = &error else {
        panic!("{error:?}")
    };
    assert_eq!(
        source.extension(),
        super::ServerHelloDisallowedExtension::Cookie
    );
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
    assert!(
        read(server_hello(
            &HELLO_RETRY_REQUEST_RANDOM.0,
            0,
            &concat(&[VERSION, &cookie])
        ))
        .is_ok()
    );
    // A HelloRetryRequest may not carry pre_shared_key; a ServerHello may.
    let psk = extension(41, &[0x00, 0x00]);
    let error = read(server_hello(
        &HELLO_RETRY_REQUEST_RANDOM.0,
        0,
        &concat(&[VERSION, &psk]),
    ))
    .unwrap_err();
    let ReadServerHelloMessageError::RetryExtensionNotAllowed { source } = &error else {
        panic!("{error:?}")
    };
    assert_eq!(
        source.extension(),
        super::HelloRetryRequestDisallowedExtension::PreSharedKey
    );
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
    assert!(read(server_hello(&[7; 32], 0, &concat(&[VERSION, &psk]))).is_ok());
    // A cookie with a byte after it is excess in a HelloRetryRequest.
    let error = read(server_hello(
        &HELLO_RETRY_REQUEST_RANDOM.0,
        0,
        &concat(&[VERSION, &extension(44, &[0x00, 0x01, 0x09, 0x00])]),
    ))
    .unwrap_err();
    assert!(matches!(
        error,
        ReadServerHelloMessageError::RetryExtensionExcess {
            extension: super::HelloRetryRequestStructuredExtension::Cookie,
            ..
        }
    ));
}

#[test]
fn field_rules() {
    let error = read(server_hello(&[7; 32], 1, VERSION)).unwrap_err();
    assert!(matches!(
        error,
        ReadServerHelloMessageError::CompressionMethodNotNull
    ));
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
    let error = read(server_hello(
        &[7; 32],
        0,
        &[0x00, 0x2b, 0x00, 0x03, 0x03, 0x04, 0x00],
    ))
    .unwrap_err();
    assert!(matches!(
        error,
        ReadServerHelloMessageError::ExtensionExcess {
            extension: super::ServerHelloStructuredExtension::SupportedVersions,
            ..
        }
    ));
    assert_eq!(error.alert(), AlertDescription::DECODE_ERROR);
    let error = read(server_hello(
        &[7; 32],
        0,
        &concat(&[&[0x00, 0x2b, 0x00, 0x01, 0x03], &extension(0x7777, &[])]),
    ))
    .unwrap_err();
    assert!(matches!(
        error,
        ReadServerHelloMessageError::SelectedVersion { .. }
    ));
}

#[test]
fn construction_rejects_the_retry_random() {
    let error = ServerHello::<&[u8]>::new(
        ProtocolVersion::TLS_1_2,
        HELLO_RETRY_REQUEST_RANDOM,
        LegacySessionId::EMPTY,
        CipherSuite::TLS_AES_128_GCM_SHA256,
        vec![super::ServerHelloExtension::SupportedVersions(
            ProtocolVersion::TLS_1_3,
        )],
    )
    .unwrap_err();
    assert!(matches!(error, InvalidServerHelloError::RetryRandom));
}

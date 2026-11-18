use super::{
    ClientHello, ClientHelloExtension, InvalidClientHelloError,
    codec::{ReadClientHelloError, read_client_hello, write_client_hello},
};
use crate::{
    alert::AlertDescription,
    cipher_suite::CipherSuite,
    code_point::{CodePointList, codec::ReadCodePointListError},
    extension::{
        ExtensionType, InvalidExtensionBlockError, OpaqueData, ReadExtensionBlockError,
        UnknownExtension,
        key_share::codec::ReadClientSharesError,
        pre_shared_key::{OfferedPsks, PskBinderEntry, PskIdentity},
    },
    message::{HelloRandom, LegacySessionId, ReadHeaderError},
    test_support::{VERSIONS, client_hello, client_hello_body, concat, extension, message},
    vector::ReadBoundedVectorError,
    version::ProtocolVersion,
};

fn read(bytes: &[u8]) -> Result<ClientHello<&[u8]>, ReadClientHelloError> {
    let mut input = bytes;
    read_client_hello(&mut input)
}

fn versions() -> Vec<u8> {
    extension(43, VERSIONS)
}

#[test]
fn a_minimal_hello_reads_and_writes_back() {
    let bytes = client_hello(&versions());
    let hello = read(&bytes).unwrap();
    assert_eq!(hello.extensions().len(), 1);
    let mut output = Vec::new();
    write_client_hello(&mut output, &hello);
    assert_eq!(output, bytes);
}

#[test]
fn another_message_type_is_unexpected() {
    let mut bytes = client_hello(&versions());
    bytes[0] = 2;
    let error = read(&bytes).unwrap_err();
    assert!(matches!(
        error,
        ReadClientHelloError::Header {
            source: ReadHeaderError::UnexpectedType { .. }
        }
    ));
    assert_eq!(error.alert(), AlertDescription::UNEXPECTED_MESSAGE);
}

#[test]
fn a_short_body_and_excess_bytes_are_decode_errors() {
    let bytes = client_hello(&versions());
    let error = read(&bytes[..bytes.len() - 1]).unwrap_err();
    assert!(matches!(
        error,
        ReadClientHelloError::Header {
            source: ReadHeaderError::Body { .. }
        }
    ));
    assert_eq!(error.alert(), AlertDescription::DECODE_ERROR);
    let body = concat(&[
        &client_hello_body(&[], &[0x13, 0x01], &[0], &versions()),
        &[0],
    ]);
    let error = read(&message(1, &body)).unwrap_err();
    let ReadClientHelloError::Excess { source } = error else {
        panic!("{error:?}")
    };
    assert_eq!(source.excess().get(), 1);
}

#[test]
fn field_rules() {
    let long_session = message(
        1,
        &client_hello_body(&[0; 33], &[0x13, 0x01], &[0], &versions()),
    );
    let error = read(&long_session).unwrap_err();
    let ReadClientHelloError::SessionId {
        source: ReadBoundedVectorError::Range { source },
    } = &error
    else {
        panic!("{error:?}")
    };
    assert_eq!((source.length(), source.max()), (33, 32));
    assert_eq!(error.alert(), AlertDescription::DECODE_ERROR);

    let odd_suites = message(
        1,
        &client_hello_body(&[], &[0x13, 0x01, 0x13], &[0], &versions()),
    );
    let error = read(&odd_suites).unwrap_err();
    assert!(matches!(
        error,
        ReadClientHelloError::CipherSuites {
            source: ReadCodePointListError::OddLength { .. }
        }
    ));
    assert_eq!(error.alert(), AlertDescription::DECODE_ERROR);

    for compression in [&[1][..], &[0, 0]] {
        let bytes = message(
            1,
            &client_hello_body(&[], &[0x13, 0x01], compression, &versions()),
        );
        let error = read(&bytes).unwrap_err();
        assert!(matches!(
            error,
            ReadClientHelloError::CompressionMethodsNotNull
        ));
        assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
    }

    let error = read(&client_hello(&extension(42, &[]))).unwrap_err();
    assert!(matches!(
        error,
        ReadClientHelloError::ExtensionsLength { .. }
    ));
}

#[test]
fn a_type_may_appear_once() {
    let bytes = client_hello(&concat(&[
        &versions(),
        &extension(0x1c, &[0x40, 0x01]),
        &versions(),
    ]));
    let error = read(&bytes).unwrap_err();
    let ReadClientHelloError::Extensions {
        source: ReadExtensionBlockError::Duplicate { source },
    } = &error
    else {
        panic!("{error:?}")
    };
    assert_eq!(source.extension(), ExtensionType::SUPPORTED_VERSIONS);
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
    // Unknown types too.
    let bytes = client_hello(&concat(&[
        &versions(),
        &extension(0x1c, &[]),
        &extension(0x1c, &[]),
    ]));
    assert_eq!(
        read(&bytes).unwrap_err().alert(),
        AlertDescription::ILLEGAL_PARAMETER
    );
}

/// pre_shared_key data with one identity and one binder.
fn offered_psks() -> Vec<u8> {
    concat(&[
        &[0x00, 0x07, 0x00, 0x01, 0xaa, 0, 0, 0, 1],
        &[0x00, 0x21, 0x20],
        &[0x55; 32],
    ])
}

#[test]
fn pre_shared_key_must_be_last() {
    let bytes = client_hello(&concat(&[&versions(), &extension(41, &offered_psks())]));
    let hello = read(&bytes).unwrap();
    assert_eq!(hello.binders_len(), Some(35));
    let bytes = client_hello(&concat(&[&extension(41, &offered_psks()), &versions()]));
    let error = read(&bytes).unwrap_err();
    assert!(matches!(error, ReadClientHelloError::PreSharedKeyNotLast));
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
}

#[test]
fn extension_data_is_checked() {
    let bytes = client_hello(&concat(&[&versions(), &extension(42, &[0])]));
    let error = read(&bytes).unwrap_err();
    let ReadClientHelloError::ExtensionExcess {
        extension: excess,
        source,
    } = error
    else {
        panic!("{error:?}")
    };
    assert_eq!(
        (excess, source.excess().get()),
        (super::ClientHelloStructuredExtension::EarlyData, 1)
    );

    let share = [0x00, 0x1d, 0x00, 0x01, 0x09];
    let shares = concat(&[&[0x00, 0x0a], &share, &share]);
    let bytes = client_hello(&concat(&[&versions(), &extension(51, &shares)]));
    let error = read(&bytes).unwrap_err();
    assert!(matches!(
        error,
        ReadClientHelloError::KeyShare {
            source: ReadClientSharesError::DuplicateGroup { .. }
        }
    ));
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);

    // A list longer than the extension data.
    let bytes = client_hello(&concat(&[
        &versions(),
        &extension(10, &[0x00, 0x04, 0x00, 0x1d]),
    ]));
    let error = read(&bytes).unwrap_err();
    assert!(matches!(
        error,
        ReadClientHelloError::SupportedGroups {
            source: ReadCodePointListError::Items { .. }
        }
    ));
}

#[test]
fn opaque_and_unknown_extensions_are_kept() {
    let bytes = client_hello(&concat(&[
        &versions(),
        &extension(57, &[1, 2, 3]),
        &extension(0xfe0d, &[4]),
        &extension(0x7777, &[5, 6]),
    ]));
    let hello = read(&bytes).unwrap();
    let ClientHelloExtension::QuicTransportParameters(parameters) = &hello.extensions()[1] else {
        panic!()
    };
    assert_eq!(*parameters.data(), [1, 2, 3]);
    let ClientHelloExtension::EncryptedClientHello(ech) = &hello.extensions()[2] else {
        panic!()
    };
    assert_eq!(*ech.data(), [4]);
    let ClientHelloExtension::Unknown(unknown) = &hello.extensions()[3] else {
        panic!()
    };
    assert_eq!(
        (unknown.extension_type(), *unknown.data()),
        (ExtensionType::new(0x7777), &[5, 6][..])
    );
}

#[test]
fn construction_checks_the_block() {
    let suites = CodePointList::new(vec![CipherSuite::TLS_AES_128_GCM_SHA256]).unwrap();
    let versions = CodePointList::new(vec![ProtocolVersion::TLS_1_3]).unwrap();
    let new = |extensions| {
        ClientHello::<&[u8]>::new(
            ProtocolVersion::TLS_1_2,
            HelloRandom([0; 32]),
            LegacySessionId::EMPTY,
            suites.clone(),
            extensions,
        )
    };
    let psk = OfferedPsks::new(
        vec![PskIdentity::new(&[1][..], 0).unwrap()],
        vec![PskBinderEntry::new(&[0; 32][..]).unwrap()],
    )
    .unwrap();
    let error = new(vec![
        ClientHelloExtension::PreSharedKey(psk),
        ClientHelloExtension::SupportedVersions(versions.clone()),
    ])
    .unwrap_err();
    assert!(matches!(
        error,
        InvalidClientHelloError::PreSharedKeyNotLast
    ));
    let error = new(vec![
        ClientHelloExtension::SupportedVersions(versions.clone()),
        ClientHelloExtension::SupportedVersions(versions.clone()),
    ])
    .unwrap_err();
    assert!(matches!(
        error,
        InvalidClientHelloError::Extensions {
            source: InvalidExtensionBlockError::Duplicate { .. }
        }
    ));
    let error = new(vec![ClientHelloExtension::EarlyData]).unwrap_err();
    assert!(matches!(
        error,
        InvalidClientHelloError::Extensions {
            source: InvalidExtensionBlockError::Length { .. }
        }
    ));
    let long = vec![0; 0x1_0000];
    assert_eq!(OpaqueData::new(&long[..]).unwrap_err().length(), 0x1_0000);
    assert!(OpaqueData::new(&long[1..]).is_ok());
    assert!(UnknownExtension::new(ExtensionType::KEY_SHARE, &[][..]).is_err());
}

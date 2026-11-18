use super::{
    EncryptedExtensionsExtension,
    codec::{ReadEncryptedExtensionsError, read_encrypted_extensions, write_encrypted_extensions},
};
use crate::{
    alert::AlertDescription,
    extension::alpn::codec::ReadSelectedProtocolError,
    test_support::{concat, extension, message, vector},
};

fn read(
    extensions: &[u8],
) -> Result<super::EncryptedExtensions<Vec<u8>>, ReadEncryptedExtensionsError> {
    let bytes = message(8, &vector(2, extensions));
    let mut input = bytes.as_slice();
    read_encrypted_extensions(&mut input).map(|message| {
        let mut output = Vec::new();
        write_encrypted_extensions(&mut output, &message);
        assert_eq!(output, bytes);
        super::EncryptedExtensions {
            extensions: message
                .extensions
                .into_iter()
                .map(|extension| match extension {
                    EncryptedExtensionsExtension::ApplicationLayerProtocolNegotiation(name) => {
                        EncryptedExtensionsExtension::ApplicationLayerProtocolNegotiation(
                            crate::extension::alpn::ProtocolName::new(name.name().to_vec())
                                .unwrap(),
                        )
                    }
                    EncryptedExtensionsExtension::QuicTransportParameters(data) => {
                        EncryptedExtensionsExtension::QuicTransportParameters(
                            crate::extension::OpaqueData::new(data.data().to_vec()).unwrap(),
                        )
                    }
                    EncryptedExtensionsExtension::ServerName => {
                        EncryptedExtensionsExtension::ServerName
                    }
                    EncryptedExtensionsExtension::EarlyData => {
                        EncryptedExtensionsExtension::EarlyData
                    }
                    _ => EncryptedExtensionsExtension::EarlyData,
                })
                .collect(),
        }
    })
}

#[test]
fn server_extensions_read_and_write_back() {
    let message = read(&concat(&[
        &extension(0, &[]),
        &extension(16, &[0x00, 0x03, 0x02, b'h', b'3']),
        &extension(57, &[0x01, 0x02]),
        &extension(42, &[]),
    ]))
    .unwrap();
    let EncryptedExtensionsExtension::ApplicationLayerProtocolNegotiation(name) =
        &message.extensions()[1]
    else {
        panic!()
    };
    assert_eq!(name.name(), b"h3");
    assert_eq!(
        message.quic_transport_parameters().map(Vec::as_slice),
        Some(&[1, 2][..])
    );
}

#[test]
fn a_client_extension_is_not_allowed() {
    let error = read(&extension(51, &[0x00, 0x00])).unwrap_err();
    assert!(
        matches!(error, ReadEncryptedExtensionsError::ExtensionNotAllowed { source } if source.extension() == super::EncryptedExtensionsDisallowedExtension::KeyShare)
    );
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
}

#[test]
fn the_selected_protocol_is_one_name() {
    let error = read(&extension(
        16,
        &[0x00, 0x06, 0x02, b'h', b'3', 0x02, b'h', b'2'],
    ))
    .unwrap_err();
    let ReadEncryptedExtensionsError::ApplicationLayerProtocolNegotiation {
        source: ReadSelectedProtocolError::NotSingle { source },
    } = &error
    else {
        panic!("{error:?}")
    };
    assert_eq!(source.count(), 2);
    assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
    let error = read(&extension(16, &[0x00, 0x01, 0x00])).unwrap_err();
    assert_eq!(error.alert(), AlertDescription::DECODE_ERROR);
}

#[test]
fn server_name_is_empty() {
    let error = read(&extension(0, &[0])).unwrap_err();
    assert!(matches!(
        error,
        ReadEncryptedExtensionsError::ExtensionExcess {
            extension: super::EncryptedExtensionsStructuredExtension::ServerName,
            ..
        }
    ));
}

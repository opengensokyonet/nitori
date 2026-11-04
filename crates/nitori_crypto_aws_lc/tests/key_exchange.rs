use aws_lc_rs::{
    agreement::{self, PrivateKey, UnparsedPublicKey, X25519},
    kem::{Ciphertext, DecapsulationKey, EncapsulationKey, ML_KEM_768},
};
use nitori_crypto::{CompleteError, KeyExchange, NamedGroup, ServerShareError, StartedExchange};
use nitori_crypto_aws_lc::AwsLcBackend;

#[test]
fn every_group_agrees() {
    let backend = AwsLcBackend;
    for (group, client_len, server_len, secret_len) in [
        (NamedGroup::X25519MLKEM768, 1216, 1120, 64),
        (NamedGroup::X25519, 32, 32, 32),
        (NamedGroup::SECP256R1, 65, 65, 32),
        (NamedGroup::SECP384R1, 97, 97, 48),
        (NamedGroup::SECP521R1, 133, 133, 66),
        (NamedGroup::MLKEM768, 1184, 1088, 32),
    ] {
        let supported = backend.named_group(group).unwrap();
        let client = KeyExchange::start(&backend, supported).unwrap();
        assert_eq!(client.group(), group);
        assert_eq!(client.share().as_bytes().len(), client_len, "{group:?}");
        let (server_share, server_secret) = backend
            .server_share(supported, client.share().as_bytes())
            .unwrap();
        assert_eq!(server_share.as_bytes().len(), server_len, "{group:?}");
        let client_secret = client.complete(server_share.as_bytes()).unwrap();
        assert_eq!(client_secret.as_bytes(), server_secret.as_bytes());
        assert_eq!(client_secret.len(), secret_len, "{group:?}");
    }
}

#[test]
fn listed_groups_are_supported() {
    let backend = AwsLcBackend;
    for &group in backend.named_groups() {
        assert!(backend.named_group(group).is_some(), "{group:?}");
    }
    assert_eq!(backend.named_groups().len(), 6);
}

#[test]
fn unsupported_groups_have_no_credential() {
    let backend = AwsLcBackend;
    // x448, SecP256r1MLKEM768 and an unassigned code point.
    for group in [
        NamedGroup::new(0x001e),
        NamedGroup::new(0x11eb),
        NamedGroup::new(0x9999),
    ] {
        assert!(backend.named_group(group).is_none(), "{group:?}");
    }
}

/// The client's share is the encapsulation key followed by the X25519
/// share, and the shared secret puts the ML-KEM secret first: a server
/// built from the primitives agrees with the backend's client.
#[test]
fn hybrid_client_follows_rfc_10024() {
    let backend = AwsLcBackend;
    let group = backend.named_group(NamedGroup::X25519MLKEM768).unwrap();
    let client = KeyExchange::start(&backend, group).unwrap();
    let (encapsulation_key, x25519_client) = client.share().as_bytes().split_at(1184);

    let (ciphertext, ml_kem_secret) = EncapsulationKey::new(&ML_KEM_768, encapsulation_key)
        .unwrap()
        .encapsulate()
        .unwrap();
    let server_key = PrivateKey::generate(&X25519).unwrap();
    let x25519_server = server_key.compute_public_key().unwrap();
    let x25519_secret = agreement::agree(
        &server_key,
        UnparsedPublicKey::new(&X25519, x25519_client),
        (),
        |shared| Ok(shared.to_vec()),
    )
    .unwrap();

    let server_share = [ciphertext.as_ref(), x25519_server.as_ref()].concat();
    let secret = client.complete(&server_share).unwrap();
    assert_eq!(
        secret.as_bytes(),
        [ml_kem_secret.as_ref(), &x25519_secret].concat()
    );
}

/// The server's share is the ciphertext followed by the X25519 share: a
/// client built from the primitives agrees with the backend's server.
#[test]
fn hybrid_server_follows_rfc_10024() {
    let decapsulation_key = DecapsulationKey::generate(&ML_KEM_768).unwrap();
    let encapsulation_key = decapsulation_key.encapsulation_key().unwrap();
    let client_key = PrivateKey::generate(&X25519).unwrap();
    let client_share = [
        encapsulation_key.key_bytes().unwrap().as_ref(),
        client_key.compute_public_key().unwrap().as_ref(),
    ]
    .concat();

    let backend = AwsLcBackend;
    let group = backend.named_group(NamedGroup::X25519MLKEM768).unwrap();
    let (server_share, secret) = backend.server_share(group, &client_share).unwrap();
    let (ciphertext, x25519_server) = server_share.as_bytes().split_at(1088);

    let ml_kem_secret = decapsulation_key
        .decapsulate(Ciphertext::from(ciphertext))
        .unwrap();
    let x25519_secret = agreement::agree(
        &client_key,
        UnparsedPublicKey::new(&X25519, x25519_server),
        (),
        |shared| Ok(shared.to_vec()),
    )
    .unwrap();
    assert_eq!(
        secret.as_bytes(),
        [ml_kem_secret.as_ref(), &x25519_secret].concat()
    );
}

#[test]
fn hybrid_shares_of_the_wrong_length_are_invalid() {
    let backend = AwsLcBackend;
    let group = backend.named_group(NamedGroup::X25519MLKEM768).unwrap();
    let client = KeyExchange::start(&backend, group).unwrap();
    let share = client.share().as_bytes();
    for len in [1215, 1217, 1184, 32] {
        let client_share = [share, &[0; 1]].concat();
        assert!(matches!(
            backend.server_share(group, &client_share[..len]),
            Err(ServerShareError::InvalidPeerShare { .. })
        ));
    }
    let (server_share, _) = backend.server_share(group, share).unwrap();
    assert!(matches!(
        client.complete(&server_share.as_bytes()[..1119]),
        Err(CompleteError::InvalidPeerShare { .. })
    ));
}

#[test]
fn hybrid_rejects_a_low_order_x25519_share() {
    let backend = AwsLcBackend;
    let group = backend.named_group(NamedGroup::X25519MLKEM768).unwrap();
    let client = KeyExchange::start(&backend, group).unwrap();
    let mut client_share = client.share().as_bytes().to_vec();
    client_share[1184..].fill(0);
    assert!(matches!(
        backend.server_share(group, &client_share),
        Err(ServerShareError::InvalidPeerShare { .. })
    ));
    let (server_share, _) = backend
        .server_share(group, client.share().as_bytes())
        .unwrap();
    let mut server_share = server_share.as_bytes().to_vec();
    server_share[1088..].fill(0);
    assert!(matches!(
        client.complete(&server_share),
        Err(CompleteError::InvalidPeerShare { .. })
    ));
}

/// FIPS 203 §7.2: every coefficient of the encapsulation key is below
/// q = 3329. A key whose first 12-bit coefficient is 4095 fails the check.
#[test]
fn ml_kem_rejects_an_encapsulation_key_out_of_range() {
    let backend = AwsLcBackend;
    for (group, offset) in [(NamedGroup::MLKEM768, 0), (NamedGroup::X25519MLKEM768, 0)] {
        let supported = backend.named_group(group).unwrap();
        let client = KeyExchange::start(&backend, supported).unwrap();
        let mut client_share = client.share().as_bytes().to_vec();
        client_share[offset] = 0xff;
        client_share[offset + 1] |= 0x0f;
        assert!(
            matches!(
                backend.server_share(supported, &client_share),
                Err(ServerShareError::InvalidPeerShare { .. })
            ),
            "{group:?}"
        );
    }
}

#[test]
fn ml_kem_ciphertext_of_the_wrong_length_is_invalid() {
    let backend = AwsLcBackend;
    let group = backend.named_group(NamedGroup::MLKEM768).unwrap();
    let client = KeyExchange::start(&backend, group).unwrap();
    assert!(matches!(
        client.complete(&[0; 1087]),
        Err(CompleteError::InvalidPeerShare { .. })
    ));
}

#[test]
fn ecdh_rejects_invalid_shares() {
    let backend = AwsLcBackend;
    let x25519 = backend.named_group(NamedGroup::X25519).unwrap();
    assert!(matches!(
        backend.server_share(x25519, &[0; 32]),
        Err(ServerShareError::InvalidPeerShare { .. })
    ));
    let p521 = backend.named_group(NamedGroup::SECP521R1).unwrap();
    let client = KeyExchange::start(&backend, p521).unwrap();
    let mut off_curve = client.share().as_bytes().to_vec();
    off_curve[132] ^= 1;
    assert!(matches!(
        backend.server_share(p521, &off_curve),
        Err(ServerShareError::InvalidPeerShare { .. })
    ));
    assert!(matches!(
        client.complete(&off_curve),
        Err(CompleteError::InvalidPeerShare { .. })
    ));
}

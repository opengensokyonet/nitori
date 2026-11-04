use nitori_crypto::{CompleteError, KeyExchange, NamedGroup, ServerShareError, StartedExchange};
use nitori_crypto_ring::RingBackend;

#[test]
fn every_group_agrees() {
    let backend = RingBackend;
    for (group, share_len, secret_len) in [
        (NamedGroup::X25519, 32, 32),
        (NamedGroup::SECP256R1, 65, 32),
        (NamedGroup::SECP384R1, 97, 48),
    ] {
        let supported = backend.named_group(group).unwrap();
        let client = KeyExchange::start(&backend, supported).unwrap();
        assert_eq!(client.group(), group);
        assert_eq!(client.share().as_bytes().len(), share_len);
        let (server_share, server_secret) = backend
            .server_share(supported, client.share().as_bytes())
            .unwrap();
        assert_eq!(server_share.as_bytes().len(), share_len);
        assert_ne!(server_share, *client.share());
        let client_secret = client.complete(server_share.as_bytes()).unwrap();
        assert_eq!(client_secret.as_bytes(), server_secret.as_bytes());
        assert_eq!(client_secret.len(), secret_len);
    }
}

#[test]
fn listed_groups_are_supported() {
    let backend = RingBackend;
    for &group in backend.named_groups() {
        assert!(backend.named_group(group).is_some(), "{group:?}");
    }
}

#[test]
fn unsupported_groups_have_no_credential() {
    let backend = RingBackend;
    for group in [
        NamedGroup::SECP521R1,
        NamedGroup::MLKEM768,
        NamedGroup::X25519MLKEM768,
        NamedGroup::new(0x9999),
    ] {
        assert!(backend.named_group(group).is_none(), "{group:?}");
        assert!(!backend.named_groups().contains(&group));
    }
}

#[test]
fn invalid_shares_are_rejected() {
    let backend = RingBackend;
    let x25519 = backend.named_group(NamedGroup::X25519).unwrap();
    // u = 0 has small order, so the shared secret would be all zeros.
    let low_order = [0; 32];
    assert!(matches!(
        backend.server_share(x25519, &low_order),
        Err(ServerShareError::InvalidPeerShare { .. })
    ));
    let client = KeyExchange::start(&backend, x25519).unwrap();
    assert!(matches!(
        client.complete(&low_order),
        Err(CompleteError::InvalidPeerShare { .. })
    ));

    let p256 = backend.named_group(NamedGroup::SECP256R1).unwrap();
    let client = KeyExchange::start(&backend, p256).unwrap();
    let mut off_curve = client.share().as_bytes().to_vec();
    off_curve[64] ^= 1;
    assert!(matches!(
        backend.server_share(p256, &off_curve),
        Err(ServerShareError::InvalidPeerShare { .. })
    ));
    assert!(matches!(
        backend.server_share(p256, &off_curve[..64]),
        Err(ServerShareError::InvalidPeerShare { .. })
    ));
}

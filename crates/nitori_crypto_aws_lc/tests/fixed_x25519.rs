#![cfg(feature = "fixed-x25519")]

use nitori_crypto::{KeyExchange, NamedGroup, StartedExchange};
use nitori_crypto_aws_lc::FixedX25519;

fn hex<const N: usize>(text: &str) -> [u8; N] {
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect();
    bytes.try_into().unwrap()
}

/// RFC 7748 §5.2: X25519 of a scalar and a u-coordinate.
#[test]
fn scalar_multiplication_matches_rfc_7748() {
    for (scalar, u, output) in [
        (
            "a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4",
            "e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c",
            "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552",
        ),
        (
            "4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d",
            "e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493",
            "95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957",
        ),
    ] {
        let fixed = FixedX25519::new(hex(scalar));
        let group = fixed.named_group(NamedGroup::X25519).unwrap();
        let client = KeyExchange::start(&fixed, group).unwrap();
        let shared = client.complete(&hex::<32>(u)).unwrap();
        assert_eq!(shared.as_bytes(), hex::<32>(output));
    }
}

/// RFC 7748 §6.1: Alice as the client and Bob as the server.
#[test]
fn diffie_hellman_matches_rfc_7748() {
    let alice_private = hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
    let alice_public: [u8; 32] =
        hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
    let bob_private = hex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
    let bob_public: [u8; 32] =
        hex("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
    let shared: [u8; 32] = hex("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");

    let alice = FixedX25519::new(alice_private);
    let group = alice.named_group(NamedGroup::X25519).unwrap();
    let client = KeyExchange::start(&alice, group).unwrap();
    assert_eq!(client.share().as_bytes(), alice_public);

    let bob = FixedX25519::new(bob_private);
    let group = bob.named_group(NamedGroup::X25519).unwrap();
    let (server_share, server_secret) = bob.server_share(group, &alice_public).unwrap();
    assert_eq!(server_share.as_bytes(), bob_public);
    assert_eq!(server_secret.as_bytes(), shared);

    assert_eq!(
        client.complete(server_share.as_bytes()).unwrap().as_bytes(),
        shared
    );
}

#[test]
fn only_x25519_is_supported() {
    let fixed = FixedX25519::new([1; 32]);
    assert_eq!(fixed.named_groups(), [NamedGroup::X25519]);
    assert!(fixed.named_group(NamedGroup::SECP256R1).is_none());
    assert!(fixed.named_group(NamedGroup::X25519MLKEM768).is_none());
}

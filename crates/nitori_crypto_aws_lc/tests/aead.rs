use nitori_crypto::{Aead, AeadAlgorithm, AeadKey, AeadOpenError};
use nitori_crypto_aws_lc::AwsLcBackend;

fn hex(text: &str) -> Vec<u8> {
    let text: String = text.split_whitespace().collect();
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

const ALGORITHMS: [AeadAlgorithm; 3] = [
    AeadAlgorithm::Aes128Gcm,
    AeadAlgorithm::Aes256Gcm,
    AeadAlgorithm::ChaCha20Poly1305,
];
const NONCE: [u8; 12] = [7; 12];
const AAD: &[u8] = b"header";
const TEXT: &[u8] = b"the quick brown fox";

fn key_of(backend: &AwsLcBackend, algorithm: AeadAlgorithm) -> impl AeadKey {
    let supported = backend.aead_algorithm(algorithm).unwrap();
    backend
        .aead_key(supported, &vec![0x42; algorithm.key_len()])
        .unwrap()
}

#[test]
fn seal_then_open_round_trips() {
    let backend = AwsLcBackend;
    for algorithm in ALGORITHMS {
        let key = key_of(&backend, algorithm);
        let mut buffer = TEXT.to_vec();
        let tag = key.seal_in_place(&NONCE, AAD, &mut buffer).unwrap();
        assert_ne!(buffer, TEXT);
        key.open_in_place(&NONCE, AAD, &mut buffer, &tag).unwrap();
        assert_eq!(buffer, TEXT);
    }
}

#[test]
fn empty_text_round_trips() {
    let backend = AwsLcBackend;
    for algorithm in ALGORITHMS {
        let key = key_of(&backend, algorithm);
        let tag = key.seal_in_place(&NONCE, AAD, &mut []).unwrap();
        key.open_in_place(&NONCE, AAD, &mut [], &tag).unwrap();
    }
}

#[test]
fn altered_input_fails_authentication() {
    let backend = AwsLcBackend;
    for algorithm in ALGORITHMS {
        let key = key_of(&backend, algorithm);
        let mut sealed = TEXT.to_vec();
        let tag = key.seal_in_place(&NONCE, AAD, &mut sealed).unwrap();

        let mut text = sealed.clone();
        text[0] ^= 1;
        let result = key.open_in_place(&NONCE, AAD, &mut text, &tag);
        assert!(matches!(result, Err(AeadOpenError::Authentication { .. })));

        let mut bad_tag = tag;
        bad_tag[15] ^= 1;
        let mut text = sealed.clone();
        let result = key.open_in_place(&NONCE, AAD, &mut text, &bad_tag);
        assert!(matches!(result, Err(AeadOpenError::Authentication { .. })));

        let mut text = sealed.clone();
        let result = key.open_in_place(&NONCE, b"headeR", &mut text, &tag);
        assert!(matches!(result, Err(AeadOpenError::Authentication { .. })));

        let mut text = sealed.clone();
        let result = key.open_in_place(&[8; 12], AAD, &mut text, &tag);
        assert!(matches!(result, Err(AeadOpenError::Authentication { .. })));
    }
}

#[test]
fn wrong_key_length_is_rejected() {
    let backend = AwsLcBackend;
    for algorithm in ALGORITHMS {
        let supported = backend.aead_algorithm(algorithm).unwrap();
        for len in [0, algorithm.key_len() - 1, algorithm.key_len() + 1] {
            let error = backend.aead_key(supported, &vec![0; len]).unwrap_err();
            assert_eq!(error.algorithm(), algorithm);
            assert_eq!(error.requested_len(), len);
        }
    }
}

#[test]
fn debug_does_not_print_the_key() {
    let backend = AwsLcBackend;
    let supported = backend.aead_algorithm(AeadAlgorithm::Aes128Gcm).unwrap();
    let key = backend.aead_key(supported, &[0x42; 16]).unwrap();
    let text = format!("{key:?}");
    assert!(text.contains("AwsLcAeadKey"));
    assert!(!text.contains("42"));
}

#[test]
fn quic_retry_integrity_tag_matches_rfc_9001_a_4() {
    let backend = AwsLcBackend;
    let supported = backend.aead_algorithm(AeadAlgorithm::Aes128Gcm).unwrap();
    let key = backend
        .aead_key(supported, &hex("be0c690b9f66575a1d766b54e368c84e"))
        .unwrap();
    let nonce: [u8; 12] = hex("461599d35d632bf2239825bb").try_into().unwrap();
    // The original destination connection ID, then the Retry packet
    // without its tag.
    let aad = hex("08 8394c8f03e515708
         ff000000010008f067a5502a4262b574 6f6b656e");
    let tag = key.seal_in_place(&nonce, &aad, &mut []).unwrap();
    assert_eq!(tag.to_vec(), hex("04a265ba2eff4d829058fb3f0f2496ba"));
    key.open_in_place(&nonce, &aad, &mut [], &tag).unwrap();
}

#[test]
fn matches_ring_for_every_algorithm() {
    let aws_lc = AwsLcBackend;
    let ring = nitori_crypto_ring::RingBackend;
    for algorithm in ALGORITHMS {
        let key_bytes: Vec<u8> = (0..algorithm.key_len() as u8).collect();
        let a = aws_lc
            .aead_key(aws_lc.aead_algorithm(algorithm).unwrap(), &key_bytes)
            .unwrap();
        let r = ring
            .aead_key(ring.aead_algorithm(algorithm).unwrap(), &key_bytes)
            .unwrap();
        let mut from_aws_lc = TEXT.to_vec();
        let mut from_ring = TEXT.to_vec();
        let aws_lc_tag = a.seal_in_place(&NONCE, AAD, &mut from_aws_lc).unwrap();
        let ring_tag = r.seal_in_place(&NONCE, AAD, &mut from_ring).unwrap();
        assert_eq!(from_aws_lc, from_ring);
        assert_eq!(aws_lc_tag, ring_tag);
        r.open_in_place(&NONCE, AAD, &mut from_aws_lc, &aws_lc_tag)
            .unwrap();
        assert_eq!(from_aws_lc, TEXT);
    }
}

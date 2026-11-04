use nitori_crypto::{ExpandLength, Hash, HashAlgorithm, Hkdf, Hmac, Random, RunningHash};
use nitori_crypto_aws_lc::AwsLcBackend;

fn hex(text: &str) -> Vec<u8> {
    let text: String = text.split_whitespace().collect();
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const ABC_SHA384: &str = "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded163\
                          1a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7";

#[test]
fn hashes_match_known_outputs() {
    let backend = AwsLcBackend;
    for (algorithm, expected) in [
        (HashAlgorithm::Sha256, ABC_SHA256),
        (HashAlgorithm::Sha384, ABC_SHA384),
    ] {
        let supported = backend.hash_algorithm(algorithm).unwrap();
        let mut running = Hash::start(&backend, supported);
        running.update(b"a");
        running.update(b"bc");
        let digest = running.finish();
        assert_eq!(digest.as_bytes(), hex(expected));
        assert_eq!(digest.as_bytes().len(), algorithm.output_len());
    }
}

#[test]
fn cloning_a_running_hash_forks_it() {
    let backend = AwsLcBackend;
    let sha256 = backend.hash_algorithm(HashAlgorithm::Sha256).unwrap();
    let mut running = Hash::start(&backend, sha256);
    running.update(b"ab");
    let mut fork = running.clone();
    fork.update(b"x");
    running.update(b"c");
    assert_eq!(running.finish().as_bytes(), hex(ABC_SHA256));
    let mut abx = Hash::start(&backend, sha256);
    abx.update(b"abx");
    assert_eq!(fork.finish(), abx.finish());
}

#[test]
fn hmac_verifies_its_own_tag_only() {
    let backend = AwsLcBackend;
    for algorithm in [HashAlgorithm::Sha256, HashAlgorithm::Sha384] {
        let supported = backend.hash_algorithm(algorithm).unwrap();
        let tag = Hmac::sign(&backend, supported, b"key", b"message");
        assert_eq!(tag.as_bytes().len(), algorithm.output_len());
        Hmac::verify(&backend, supported, b"key", b"message", tag.as_bytes()).unwrap();
        assert!(Hmac::verify(&backend, supported, b"key", b"massage", tag.as_bytes()).is_err());
        assert!(Hmac::verify(&backend, supported, b"kex", b"message", tag.as_bytes()).is_err());
        let truncated = &tag.as_bytes()[..tag.as_bytes().len() - 1];
        assert!(Hmac::verify(&backend, supported, b"key", b"message", truncated).is_err());
    }
}

/// RFC 5869 appendix A.1 to A.3: the SHA-256 cases.
///
/// Expand outputs are limited to 64 bytes, and a shorter HKDF output is a
/// prefix of a longer one, so case 2 compares the first 64 of its 82 bytes.
#[test]
fn hkdf_matches_rfc_5869() {
    struct Case {
        ikm: &'static str,
        salt: &'static str,
        info: &'static str,
        prk: &'static str,
        okm: &'static str,
    }
    let cases = [
        Case {
            ikm: "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b",
            salt: "000102030405060708090a0b0c",
            info: "f0f1f2f3f4f5f6f7f8f9",
            prk: "077709362c2e32df0ddc3f0dc47bba63 90b6c73bb50f9c3122ec844ad7c2b3e5",
            okm: "3cb25f25faacd57a90434f64d0362f2a 2d2d0a90cf1a5a4c5db02d56ecc4c5bf
                  34007208d5b887185865",
        },
        Case {
            ikm: "000102030405060708090a0b0c0d0e0f 101112131415161718191a1b1c1d1e1f
                  202122232425262728292a2b2c2d2e2f 303132333435363738393a3b3c3d3e3f
                  404142434445464748494a4b4c4d4e4f",
            salt: "606162636465666768696a6b6c6d6e6f 707172737475767778797a7b7c7d7e7f
                   808182838485868788898a8b8c8d8e8f 909192939495969798999a9b9c9d9e9f
                   a0a1a2a3a4a5a6a7a8a9aaabacadaeaf",
            info: "b0b1b2b3b4b5b6b7b8b9babbbcbdbebf c0c1c2c3c4c5c6c7c8c9cacbcccdcecf
                   d0d1d2d3d4d5d6d7d8d9dadbdcdddedf e0e1e2e3e4e5e6e7e8e9eaebecedeeef
                   f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff",
            prk: "06a6b88c5853361a06104c9ceb35b45c ef760014904671014a193f40c15fc244",
            okm: "b11e398dc80327a1c8e7f78c596a4934 4f012eda2d4efad8a050cc4c19afa97c
                  59045a99cac7827271cb41c65e590e09 da3275600c2f09b8367793a9aca3db71",
        },
        Case {
            ikm: "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b",
            salt: "",
            info: "",
            prk: "19ef24a32c717b167f33a91d6f648bdf 96596776afdb6377ac434c1c293ccb04",
            okm: "8da4e775a563c18f715f802a063c5a31 b8a11f5c5ee1879ec3454e5f3c738d2d
                  9d201395faa4b61a96c8",
        },
    ];
    let backend = AwsLcBackend;
    let sha256 = backend.hash_algorithm(HashAlgorithm::Sha256).unwrap();
    for case in cases {
        let prk = backend.extract(sha256, &hex(case.salt), &hex(case.ikm));
        assert_eq!(prk.as_bytes(), hex(case.prk));
        let okm = hex(case.okm);
        let len = ExpandLength::new(okm.len().try_into().unwrap()).unwrap();
        let info = hex(case.info);
        // The info is given in two pieces, which HKDF concatenates.
        let (first, second) = info.split_at(info.len() / 2);
        let out = backend.expand(sha256, &prk, &[first, second], len);
        assert_eq!(out.as_bytes(), okm);
    }
}

#[test]
fn expand_takes_any_length_from_1_to_64() {
    let backend = AwsLcBackend;
    let sha384 = backend.hash_algorithm(HashAlgorithm::Sha384).unwrap();
    let prk = backend.extract(sha384, b"salt", b"ikm");
    assert_eq!(prk.len(), 48);
    let longest = backend.expand(sha384, &prk, &[b"info"], ExpandLength::MAX);
    for len in 1..=64 {
        let out = backend.expand(sha384, &prk, &[b"info"], ExpandLength::new(len).unwrap());
        assert_eq!(out.as_bytes(), &longest.as_bytes()[..usize::from(len)]);
    }
}

#[test]
fn random_fills_arrays() {
    let backend = AwsLcBackend;
    let first: [u8; 32] = backend.random().unwrap();
    let second: [u8; 32] = backend.random().unwrap();
    assert_ne!(first, second);
    let empty: [u8; 0] = backend.random().unwrap();
    assert_eq!(empty, []);
}

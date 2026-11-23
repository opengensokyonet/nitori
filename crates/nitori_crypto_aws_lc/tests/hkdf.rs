//! HKDF-Expand takes a PRK of any length a secret holds.

use nitori_crypto::{ExpandLength, Hash, HashAlgorithm, Hkdf, Secret};
use nitori_crypto_aws_lc::AwsLcBackend;
use nitori_crypto_ring::RingBackend;

#[test]
fn expand_takes_a_prk_longer_than_a_hash() {
    // A P-521 shared secret is 66 bytes, longer than the longest hash.
    let prk = Secret::new(&[7; Secret::CAPACITY]).unwrap();
    for algorithm in [HashAlgorithm::Sha256, HashAlgorithm::Sha384] {
        let r = RingBackend.hash_algorithm(algorithm).unwrap();
        let a = AwsLcBackend.hash_algorithm(algorithm).unwrap();
        // One block, part of a block and two blocks of both hashes.
        for len in [1, 32, 48, 64] {
            let len = ExpandLength::new(len).unwrap();
            assert_eq!(
                RingBackend.expand(r, &prk, &[b"in", b"fo"], len).as_bytes(),
                AwsLcBackend.expand(a, &prk, &[b"info"], len).as_bytes(),
                "{algorithm:?}, {} bytes",
                len.get()
            );
        }
    }
}

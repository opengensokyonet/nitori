# nitori_crypto_aws_lc

The capabilities of `nitori_crypto` implemented with
[aws-lc-rs](https://crates.io/crates/aws-lc-rs) 1.19. It is experimental
and builds on stable Rust. aws-lc-rs compiles AWS-LC from C sources, which
needs a C compiler and CMake.

`AwsLcBackend` is a zero-sized type that implements `Hash`, `Hmac`, `Hkdf`,
`KeyExchange`, `SignatureVerify` and `Random`:

| Capability | Algorithms |
| --- | --- |
| `Hash`, `Hmac`, `Hkdf` | SHA-256, SHA-384 |
| `KeyExchange` | X25519MLKEM768, x25519, secp256r1, secp384r1, secp521r1, MLKEM768 |
| `SignatureVerify` | ecdsa_secp256r1_sha256, ecdsa_secp384r1_sha384, ecdsa_secp521r1_sha512, ed25519, rsa_pss_rsae_sha256/384/512, rsa_pkcs1_sha256/384/512 |
| `Random` | AWS-LC's generator |

X25519MLKEM768 follows RFC 10024: the client's share is the ML-KEM-768
encapsulation key followed by the X25519 share, the server's share is the
ML-KEM ciphertext followed by the X25519 share, and the shared secret is
the ML-KEM shared secret followed by the X25519 shared secret. RSA
verification accepts moduli of 2048 to 8192 bits; the rsa_pkcs1 schemes are
for signatures in certificates.

`EcdsaSigningKey`, `Ed25519SigningKey` and `RsaSigningKey` are local
private keys built from PKCS#8 documents. They implement `SigningKey`, and
their signatures complete immediately. An RSA key signs with RSA-PSS only.

With the `fixed-x25519` feature, `FixedX25519` exchanges X25519 keys with a
private key given at construction, to reproduce published traces such as
those of RFC 8448. It implements `KeyExchange` only; combine it with
`AwsLcBackend` in a type that delegates each capability.

```rust
use nitori_crypto::{KeyExchange, NamedGroup, StartedExchange};
use nitori_crypto_aws_lc::AwsLcBackend;

let backend = AwsLcBackend;
let group = backend.named_group(NamedGroup::X25519MLKEM768).unwrap();
let client = KeyExchange::start(&backend, group).unwrap();
assert_eq!(client.share().as_bytes().len(), 1216);
let (server_share, server_secret) = backend.server_share(group, client.share().as_bytes()).unwrap();
assert_eq!(server_share.as_bytes().len(), 1120);
let client_secret = client.complete(server_share.as_bytes()).unwrap();
assert_eq!(client_secret.as_bytes(), server_secret.as_bytes());
```

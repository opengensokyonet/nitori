# nitori_crypto_ring

The capabilities of `nitori_crypto` implemented with
[ring](https://crates.io/crates/ring) 0.17. It is experimental and builds on
stable Rust.

`RingBackend` is a zero-sized type that implements `Hash`, `Hmac`, `Hkdf`,
`KeyExchange`, `SignatureVerify` and `Random`:

| Capability | Algorithms |
| --- | --- |
| `Hash`, `Hmac`, `Hkdf` | SHA-256, SHA-384 |
| `KeyExchange` | x25519, secp256r1, secp384r1 |
| `SignatureVerify` | ecdsa_secp256r1_sha256, ecdsa_secp384r1_sha384, ed25519, rsa_pss_rsae_sha256/384/512, rsa_pkcs1_sha256/384/512 |
| `Random` | the operating system's generator |

RSA verification accepts moduli of 2048 to 8192 bits. The rsa_pkcs1 schemes
are for signatures in certificates; TLS 1.3 does not use them in handshake
messages. HKDF-Extract is computed as HMAC, so that the pseudorandom key is
a `Secret` like every other output.

`EcdsaSigningKey`, `Ed25519SigningKey` and `RsaSigningKey` are local
private keys built from PKCS#8 documents. They implement `SigningKey`, and
their signatures complete immediately. An RSA key signs with RSA-PSS only.

```rust
use nitori_crypto::{KeyExchange, NamedGroup, StartedExchange};
use nitori_crypto_ring::RingBackend;

let backend = RingBackend;
let group = backend.named_group(NamedGroup::X25519).unwrap();
let client = KeyExchange::start(&backend, group).unwrap();
let (server_share, server_secret) = backend.server_share(group, client.share().as_bytes()).unwrap();
let client_secret = client.complete(server_share.as_bytes()).unwrap();
assert_eq!(client_secret.as_bytes(), server_secret.as_bytes());
assert!(backend.named_group(NamedGroup::X25519MLKEM768).is_none());
```

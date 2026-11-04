# nitori_crypto

Cryptographic capabilities as traits, one trait per capability, with the
TLS code points that name their algorithms. It is experimental and builds
on stable Rust; the repository still pins a nightly toolchain for its other
crates. The crate has no implementations: `nitori_crypto_ring` and
`nitori_crypto_aws_lc` implement the traits with existing libraries.

## Capabilities

- `Hash` starts a `RunningHash`; cloning a running hash forks it.
- `Hmac` signs and verifies HMAC tags; verification compares in constant
  time.
- `Hkdf` performs HKDF-Extract and HKDF-Expand. Constructions built on top,
  such as the TLS 1.3 `HKDF-Expand-Label`, belong to the protocol layer.
- `KeyExchange` exchanges keys by `NamedGroup`: the server in one step, the
  client in two, where completing consumes the started exchange.
- `SignatureVerify` verifies signatures by `SignatureScheme`.
- `Random` returns random bytes as a fixed-size array.
- `SigningKey` signs; one signature is one interaction, carried by an
  associated future.

A type implements the capabilities it has. A protocol states the set it
needs as a trait with these as supertraits and implements it for every type
that has them all; a type that lacks a capability does not compile where it
is needed. A type that combines several libraries implements each trait by
delegating to the one that provides it.

## Support is answered once

Which algorithms a capability supports is answered when the algorithm is
chosen. `Hash::hash_algorithm`, `KeyExchange::named_group` and
`SignatureVerify::signature_scheme` return a `Supported<Self, A>` for an
algorithm the implementation supports and `None` otherwise. The operations
take that value and do not fail for lack of support. Only the
implementation issues these values, by the token of its `Issuer`
implementation, and a value issued for one type is not accepted by another.
A `Supported` hash algorithm is accepted by `Hmac` and `Hkdf` as well.

```rust
use nitori_crypto::{ExpandLength, Hash, HashAlgorithm, Hkdf, Hmac, RunningHash, Secret};

fn derive<C: Hash + Hmac + Hkdf>(backend: &C, ikm: &[u8], message: &[u8]) -> Option<Secret> {
    let sha256 = backend.hash_algorithm(HashAlgorithm::Sha256)?;
    let mut transcript = Hash::start(backend, sha256);
    transcript.update(message);
    let digest = transcript.finish();
    let prk = backend.extract(sha256, &[], ikm);
    let tag = Hmac::sign(backend, sha256, prk.as_bytes(), digest.as_bytes());
    Some(backend.expand(sha256, &prk, &[tag.as_bytes()], ExpandLength::hash_len(HashAlgorithm::Sha256)))
}
```

Capabilities share method names such as `start`, `sign` and `verify`. Where
one type implements several of them, calls name the trait, as in
`Hash::start(&backend, algorithm)`.

## Values

- `Secret` holds secret bytes inline and overwrites them when dropped.
- `Digest` is a hash or HMAC output.
- `ExpandLength` is an HKDF-Expand output length from 1 to 64 bytes,
  checked when it is built.
- `KeyShare` and `Signature` store common sizes inline and longer values on
  the heap. The inline capacities are tuned to the sizes in use and may
  change.

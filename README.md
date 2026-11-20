# nitori

Foundational Rust tools for Open Gensokyo Network, administratively supported by Starspun Works.

Source: [opengensokyonet/nitori](https://github.com/opengensokyonet/nitori).

This repository contains the experimental `nitori_call` mechanism, `nitori_io` byte IO capabilities and operations, `nitori_crypto` cryptographic capabilities with ring and aws-lc-rs implementations, the `nitori_tls` TLS 1.3 handshake for QUIC, and a DATA frame codec prototype. The current APIs are experimental and are not stable production APIs.

## Layout

Each package documents itself in its own README or crate documentation. Dependencies are those on other packages in this repository.

### Calls

- [`crates/nitori_call`](crates/nitori_call/README.md): operations written as ordinary async functions that run on a resource their caller lends one poll at a time; depends on `nitori_call_macros`.
- `crates/nitori_call_macros`: the `#[call]` and `#[capability]` procedural macros of `nitori_call`.

### IO

- [`crates/nitori_io`](crates/nitori_io/README.md): byte IO capabilities, operations and conversion helpers; depends on `nitori_call`.

### Identity

- [`crates/nitori_id`](crates/nitori_id/README.md): identities unique within a process and never reused; no dependencies.

### Cryptography

- [`crates/nitori_crypto`](crates/nitori_crypto/README.md): cryptographic capabilities as traits, with the TLS code points that name their algorithms; no dependencies.
- [`crates/nitori_crypto_ring`](crates/nitori_crypto_ring/README.md): the `nitori_crypto` capabilities implemented with ring; depends on `nitori_crypto`.
- [`crates/nitori_crypto_aws_lc`](crates/nitori_crypto_aws_lc/README.md): the `nitori_crypto` capabilities implemented with aws-lc-rs, including P-521 and ML-KEM; depends on `nitori_crypto`.

### TLS

- [`crates/nitori_tls`](crates/nitori_tls/README.md): the TLS 1.3 handshake for QUIC; depends on `nitori_call`, `nitori_io` and `nitori_crypto`.

### Examples and benchmarks

- `examples/data-frame-codec`: a QUIC VarInt and HTTP/3 DATA frame codec with tests, compiler boundary probes and macro expansion tooling; depends on `nitori_call` and `nitori_io`. See its crate documentation.
- `benches/call-bench`: HTTP/2 and HTTP/3 benchmarks that compare ordinary async, tokio-util codecs and `nitori_call`; depends on `nitori_call`. See its crate documentation.

## Development

Install [rustup](https://rustup.rs/) and Python 3; `nitori_crypto_aws_lc` also needs a C compiler and CMake. The repository pins `nightly-2026-11-03` with rustfmt and Clippy in `rust-toolchain.toml` (rustc 1.101.0-nightly, 1c3a65069). Run commands from the repository root so rustup selects this toolchain. Nightly language features are required. Packages remain unpublished (`publish = false`).

SNAFU is pinned to commit `03cb8d2e52a3b0b33a39cc3d4ce8a7075fa4ab58` in the public [Open Gensokyo Network fork](https://github.com/opengensokyonet/snafu), pending [upstream PR #565](https://github.com/shepmaster/snafu/pull/565). Its conditional diagnostic trait implementations and decoupled error construction are required. Cargo fetches this source automatically; no sibling checkout is needed.

```sh
git clone https://github.com/opengensokyonet/nitori.git
cd nitori
cargo fetch --locked
```

From the repository root:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --all-targets --all-features
cargo test --locked --workspace --doc
cargo test --locked --workspace --doc --all-features
cargo test --locked -p nitori_io --features tokio
cargo test --locked -p nitori_io --features futures
python3 examples/data-frame-codec/scripts/check_boundaries.py
cargo run --locked --example macro_demo
cargo run --locked --example expansion_demo
cargo run --locked -p nitori_call --example call_composition
cargo run --locked -p nitori_io --example limited_read
python3 examples/data-frame-codec/scripts/expand_example.py
```

The boundary probes run offline after Cargo has fetched workspace dependencies. Consumers of `#[call]` do not need a direct pin-projection dependency.

For local SNAFU development, an ignored `.cargo/config.toml` may patch both `snafu` and `snafu-derive` under `[patch."https://github.com/opengensokyonet/snafu"]` to a local checkout. Run `cargo metadata --format-version 1` to update the local lockfile and verify both manifest paths before using `--locked`. Keep that configuration and the resulting path-source lockfile changes out of commits; the committed lockfile records the public Git source.

## License

Software and code examples are licensed under either [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option. Explanatory documentation is licensed under [CC BY 4.0](LICENSE-CC-BY). Contributors retain copyright in their contributions. Project names and logos are not licensed as trademarks by these licenses.

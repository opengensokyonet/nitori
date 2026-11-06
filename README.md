# nitori

Foundational Rust tools for Open Gensokyo Network, administratively supported by Starspun Works.

Source: [opengensokyonet/nitori](https://github.com/opengensokyonet/nitori).

This repository contains the experimental `nitori_call` mechanism, `nitori_io` byte IO capabilities and operations, `nitori_crypto` cryptographic capabilities with ring and aws-lc-rs implementations, and a DATA frame codec prototype. The current APIs are experimental and are not stable production APIs.

## Layout

- `crates/nitori_call`: calls written as ordinary async functions that do not store what they work on; the code polling a call lends it a view of its resource, or none, on each poll. That code can then acquire a shared resource only for the polls that use it, and an owner of shared state, such as a connection, can keep many calls next to the state and lend each call in turn a view of it. The crate provides operations, receiver accesses, composition, executors, and the `#[call]` and `#[capability]` macros. Executions are both futures of the final value and streams of yielded items. A caller that lends every view on every poll gets synchronous access to them, where `.await` and `yield` are the only points other work can run. See its [README](crates/nitori_call/README.md).
- `crates/nitori_call_macros`: procedural macros used by `nitori_call`.
- `crates/nitori_crypto`: cryptographic capabilities as traits, one per capability: hashing, HMAC, HKDF, key exchange, signature verification, signing and random bytes, with the TLS code points that name their algorithms. An implementation answers which algorithms it supports with a `Supported` value when the algorithm is chosen, and operations that take one do not fail for lack of support. See its [README](crates/nitori_crypto/README.md).
- `crates/nitori_crypto_ring`: the `nitori_crypto` capabilities that [ring](https://crates.io/crates/ring) provides, and local signing keys built from PKCS#8. See its [README](crates/nitori_crypto_ring/README.md).
- `crates/nitori_crypto_aws_lc`: the `nitori_crypto` capabilities that [aws-lc-rs](https://crates.io/crates/aws-lc-rs) provides, adding P-521, ML-KEM-768 and the X25519MLKEM768 hybrid group, and local signing keys built from PKCS#8. The `fixed-x25519` feature adds an X25519 exchange with a given private key, for reproducing published traces. aws-lc-rs builds AWS-LC from C sources, which needs a C compiler and CMake. See its [README](crates/nitori_crypto_aws_lc/README.md).
- `crates/nitori_id`: identities unique within a process and never reused, each pairing the `TypeId` of its owning type with an opaque allocated value. See its [README](crates/nitori_id/README.md).
- `crates/nitori_io`: generic byte IO capabilities, calls, and explicit conversion helpers; see its [README](crates/nitori_io/README.md).
- `examples/data-frame-codec`: codec examples, behavioral tests, compiler boundary probes, and macro expansion tooling.
- `benches/call-bench`: HTTP responses from a captured header corpus, read and written with ordinary async, tokio-util codecs, and `nitori_call`, each written the way that suits it best: HTTP/2 with HPACK over a Tokio duplex pipe, and HTTP/3 with QPACK over QUIC stream buffers in user space, each behind its own lock, with one task per stream. Field sections use the static table only, with Huffman-coded strings; separate scenarios receive and send the QPACK encoder stream that builds a dynamic table from the same corpus, whose instructions carry no length, so a reader finds where one ends only by parsing it. Shared resources sit behind a fair async, an unfair async, or a blocking mutex; the source's arrival rate, the flow control update threshold and the contention on the HTTP/2 connection are options, and the report counts queued and empty lock acquisitions on each side. The work the application does per DATA byte is an option too, and a `mechanism` scenario measures the fixed cost of an access with no work behind it. HTTP/3 also runs the calls through `nitori_call`'s own lock executors. A `driver` scenario runs many calls over one state the way a connection driver does, lending the state to each call it polls, against ordinary futures sharing it through `Rc<RefCell<_>>`. The scenarios run on a current-thread runtime and on multi-thread runtimes with two and four workers, and, built with `--cfg tokio_unstable`, with two workers and the LIFO slot turned off. Run `cargo run --release -p nitori-call-bench -- <h2-read|h2-write|h3-recv|h3-send|qpack-encoder-recv|qpack-encoder-send|fields|mechanism|driver>`; see the crate documentation for the options. `scripts/gen_tables.py` regenerates the static and Huffman tables from the RFC text.

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

The boundary probes run offline after Cargo has fetched workspace dependencies. Consumers of `#[call]` do not need a direct pin-projection dependency. The codec demonstrates QUIC VarInt and single-frame HTTP/3 DATA processing; it does not implement complete HTTP/3 semantics.

For local SNAFU development, an ignored `.cargo/config.toml` may patch both `snafu` and `snafu-derive` under `[patch."https://github.com/opengensokyonet/snafu"]` to a local checkout. Run `cargo metadata --format-version 1` to update the local lockfile and verify both manifest paths before using `--locked`. Keep that configuration and the resulting path-source lockfile changes out of commits; the committed lockfile records the public Git source.

## License

Software and code examples are licensed under either [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option. Explanatory documentation is licensed under [CC BY 4.0](LICENSE-CC-BY). Contributors retain copyright in their contributions. Project names and logos are not licensed as trademarks by these licenses.

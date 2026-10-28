# nitori

Foundational Rust tools for Open Gensokyo Network, administratively supported by Starspun Works.

Source: [opengensokyonet/nitori](https://github.com/opengensokyonet/nitori).

This repository contains the experimental `nitori_call` mechanism, `nitori_io` byte IO capabilities and operations, and a DATA frame codec prototype. The current APIs are experimental and are not stable production APIs.

## Layout

- `crates/nitori_call`: calls written as ordinary async functions and polled with a view of their resource, or none, on each poll; operations, receiver accesses, composition, executors, and the `#[call]` and `#[capability]` macros. Executions are both futures of the final value and streams of yielded items. See its [README](crates/nitori_call/README.md).
- `crates/nitori_call_macros`: procedural macros used by `nitori_call`.
- `crates/nitori_io`: generic byte IO capabilities, calls, and explicit conversion helpers; see its [README](crates/nitori_io/README.md).
- `examples/data-frame-codec`: codec examples, behavioral tests, compiler boundary probes, and macro expansion tooling.
- `benches/call-bench`: HTTP responses from a captured header corpus, read and written with ordinary async, tokio-util codecs, and `nitori_call`, each written the way that suits it best: HTTP/2 with HPACK over a Tokio duplex pipe, and HTTP/3 with QPACK over QUIC stream buffers in user space, each behind its own lock, with one task per stream. Field sections use the static table only, with Huffman-coded strings; separate scenarios receive and send the QPACK encoder stream that builds a dynamic table from the same corpus, whose instructions carry no length and are mostly a few bytes long. Shared resources sit behind a fair async, an unfair async, or a blocking mutex; the source's arrival rate, the flow control update threshold and the contention on the HTTP/2 connection are options, and the report counts queued and empty lock acquisitions on each side. The work the application does per DATA byte is an option too, and a `mechanism` scenario measures the fixed cost of an access with no work behind it. Run `cargo run --release -p nitori-call-bench -- <h2-read|h2-write|h3-recv|h3-send|qpack-encoder-recv|qpack-encoder-send|fields|mechanism>`; see the crate documentation for the options. `scripts/gen_tables.py` regenerates the static and Huffman tables from the RFC text.

## Development

Install [rustup](https://rustup.rs/) and Python 3. The repository pins `nightly-2026-09-20` with rustfmt and Clippy in `rust-toolchain.toml` (rustc 1.100.0-nightly, feaadeeac). Run commands from the repository root so rustup selects this toolchain. Nightly language features are required. Packages remain unpublished (`publish = false`).

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

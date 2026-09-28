# nitori

Foundational Rust tools for Open Gensokyo Network, administratively supported by Starspun Works.

Source: [opengensokyonet/nitori](https://github.com/opengensokyonet/nitori).

This repository contains the experimental `nitori_call` mechanism, `nitori_io` byte IO capabilities and operations, and a DATA frame codec prototype. The current APIs are experimental and are not stable production APIs.

## Layout

- `crates/nitori_call`: `ReceiverFamily`, `ReceiverScope`, `ResourceDriver`, `CallOn`, composition, asynchronous and synchronous adapters, and the public macro re-exports; see its [README](crates/nitori_call/README.md) for `#[call(sync)]` and synchronous event iteration.
- `crates/nitori_call_macros`: procedural macros used by `nitori_call`.
- `crates/nitori_io`: generic byte IO capabilities, calls, and explicit conversion helpers; see its [README](crates/nitori_io/README.md).
- `examples/data-frame-codec`: codec examples, behavioral tests, compiler boundary probes, and macro expansion tooling.

## Development

Install [rustup](https://rustup.rs/) and Python 3. The repository pins `nightly-2026-09-20` with rustfmt and Clippy in `rust-toolchain.toml` (rustc 1.100.0-nightly, feaadeeac). Run commands from the repository root so rustup selects this toolchain. Nightly language features are required. Packages remain unpublished (`publish = false`).

SNAFU is pinned to commit `03cb8d2e52a3b0b33a39cc3d4ce8a7075fa4ab58` in the public [Open Gensokyo Network fork](https://github.com/opengensokyonet/snafu), pending [upstream PR #565](https://github.com/shepmaster/snafu/pull/565). Its conditional diagnostic trait implementations and independent error construction are required. Cargo fetches this source automatically; no sibling checkout is needed.

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
python3 examples/data-frame-codec/scripts/check_resume.py
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

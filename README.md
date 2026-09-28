# nitori

Foundational Rust tools for Open Gensokyo Network, administratively supported by Starspun Works.

This repository contains the experimental `nitori_call` mechanism, `nitori_io` byte IO capabilities and operations, and a DATA frame codec prototype. The current APIs are experimental and are not stable production APIs.

## Layout

- `crates/nitori_call`: `ReceiverFamily`, `ReceiverScope`, `ResourceDriver`, `CallOn`, composition, asynchronous and synchronous adapters, and the public macro re-exports; see its [README](crates/nitori_call/README.md) for `#[call(sync)]` and synchronous event iteration.
- `crates/nitori_call_macros`: procedural macros used by `nitori_call`.
- `crates/nitori_io`: generic byte IO capabilities, calls, and explicit conversion helpers; see its [README](crates/nitori_io/README.md).
- `examples/data-frame-codec`: codec examples, behavioral tests, compiler boundary probes, and macro expansion tooling.

## Development

Use Rust nightly with rustfmt and Clippy, plus Python 3. The migration was checked with rustc 1.100.0-nightly (feaadeeac, 2026-09-19). Nightly language features are required; newer toolchains may change their behavior. Packages remain unpublished (`publish = false`).

From the repository root:

```sh
cargo +nightly fmt --all --check
cargo +nightly clippy --locked --workspace --all-targets -- -D warnings
cargo +nightly test --locked --workspace --all-targets
cargo +nightly test --locked --workspace --doc
python3 examples/data-frame-codec/scripts/check_boundaries.py
python3 examples/data-frame-codec/scripts/check_resume.py
cargo +nightly run --locked --example macro_demo
cargo +nightly run --locked --example expansion_demo
python3 examples/data-frame-codec/scripts/expand_example.py
```

The boundary probes run offline after Cargo has fetched workspace dependencies. Consumers of `#[call]` do not need a direct pin-projection dependency. The codec demonstrates QUIC VarInt and single-frame HTTP/3 DATA processing; it does not implement complete HTTP/3 semantics.

## License

Software and code examples are licensed under either [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option. Explanatory documentation is licensed under [CC BY 4.0](LICENSE-CC-BY). Contributors retain copyright in their contributions. Project names and logos are not licensed as trademarks by these licenses.

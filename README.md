# nitori

Foundational Rust tools for Open Gensokyo Network, administratively supported by Starspun Works.

This repository currently contains the experimental `nitori_call` mechanism and a DATA frame codec example. Future tools such as `nitori_io` will be added as their contracts are developed. The current APIs are experimental and are not stable production APIs.

## Layout

- `crates/nitori_call`: `CallOn`, `BoundCall`, and the public macro re-exports.
- `crates/nitori_call_macros`: procedural macros used by `nitori_call`.
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

The boundary probes run offline after Cargo has fetched workspace dependencies. Consumers of `#[call]` currently need a direct `pin-project` dependency. The codec demonstrates QUIC VarInt and single-frame HTTP/3 DATA processing; it does not implement complete HTTP/3 semantics.

## License

Software and code examples are licensed under either [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option. Explanatory documentation is licensed under [CC BY 4.0](LICENSE-CC-BY). Contributors retain copyright in their contributions. Project names and logos are not licensed as trademarks by these licenses.

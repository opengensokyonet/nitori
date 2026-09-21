# Typed child-await prototype

This is a historical, baseline-pinned experiment. The [current library](../../crates/nitori_call/README.md) now provides the child-await protocol and Receiver extension traits. The results and limitations below describe the archived experiment.

This isolated experiment applies a patch to commit `eefb694aa4ce6184c1d6658f09222dc76cff5b8f` in a temporary checkout. It does not enable new APIs in the main workspace. Its purpose is to test real child values, manual pinning, and type-directed await dispatch inside `#[call]`.

The executable [test body](typed_children.rs) includes this flow:

```rust,ignore
let (first, second) = (io.decode(10), io.decode(20));
let mut container = Some(if ready(true).await { first } else { second });
let selected = identity(container.take().unwrap());
let factory = move || selected;
let selected = factory();
let mut child = pin!(selected);
let borrowed = child.as_mut();
let next = borrowed.next();
let event = identity(next).await;
```

Only virtual receiver operations and the unified await entry need special lowering. The macro does not track child declarations, aliases, containers, captures, or names. `Child` owns the operation without borrowing a Host; `Next` borrows a pinned child. Both implement the experimental `IntoAwaitOn` / `AwaitOn` protocol. Ordinary `IntoFuture` values use a blanket adapter. These implementations coexist without specialization. `Next` deliberately does **not** implement standard `Future`: it needs a Host in addition to a Context.

Direct child await discards `Yielded` values and returns `Complete`. `next().await` returns one `Some(Yielded(...))` or `Some(Complete(...))`; after completion it returns `None`. Pending propagates, and dropping the parent cancels its owned child without rolling back effects. The parent may access the Host through `.with` between child events. Ordinary local mutable input borrows can survive nested waits.

## Reproduce

Prerequisites: Python 3.12+, Git, Rust nightly with rustfmt, Clippy and Miri, plus the SNAFU checkout required by this workspace (including PR #565 changes). Tested with rustc 1.100.0-nightly (2026-09-19).

From the nitori repository root, with the workspace's sibling `snafu` checkout:

```sh
python3 experiments/typed-await/verify.py --snafu ../snafu --miri
```

`--snafu` also accepts another path. The script creates an uncommitted Cargo patch in the temporary checkout and checks that both SNAFU packages resolve there. It prints the temporary location and retains it for inspection. Omit `--miri` to skip the two Miri runs.

The runner checks formatting, workspace default and all-feature tests, all-feature doctests and Clippy, existing compiler/resume probes, and five new behavioral tests. With `--miri`, those five tests also run under the default and Tree Borrows models. Three negative cases verify rejection of an unpinned child, overlapping `next` borrows, and an escaped Host reference. Negative diagnostics are saved in the temporary checkout.

## Boundaries still open

- [return-child.rs](return-child.rs) reproduces an opaque-type inference error when another `#[call]` returns a concrete `Child<Host, Decode>`. The runner expects this failure as a known limitation, not a desired language restriction. Ordinary function passage and closure capture/return of an existing child pass.
- Receiver-specific generated extension traits are not implemented here. The prototype supports virtual Receiver syntax and real `from_pin` / `from_mut` constructors, plus short Host projections and `.with`. It retains the original Pin receiver path. It does not establish equivalence with unannotated async functions.
- The tested `sync_*` call has no yield and runs within short Host access. Persistent synchronous event iteration inside the macro remains undesigned.
- `pin!` is an explicitly recognized intrinsic spelling: the macro parses its expression and emits `::core::pin::pin!`. Arbitrary opaque macros remain unsupported; a custom macro ending in `pin` is not preserved as a user-defined macro.
- Receiver annotation parsing and method lowering are intentionally narrow. Generic sync-method arguments, complex projection arguments and exhaustive Rust syntax compatibility are not validated.
- Host type compatibility is checked; logical Host identity remains a caller contract. No performance or fairness claim is made for direct await that keeps discarding yields. Miri results cover the tested paths, not a complete soundness proof.

Implementation: [adapter types](typed.rs), [macro/runtime patch](prototype.patch), [reproduction runner](verify.py).

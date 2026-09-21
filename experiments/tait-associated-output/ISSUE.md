# Draft: TAIT whose Fn::Output is another TAIT fails with E0282; RPIT and nominal wrapper work

Status: prepared locally, not submitted. No exact duplicate has been confirmed.

## Reproduction

No dependencies, procedural macros, async code, lifetimes or explicit trait
implementations are needed:

```rust
#![feature(type_alias_impl_trait)]

type Inner = impl Sized;
#[define_opaque(Inner)]
fn inner() -> Inner { 8 }

type Outer = impl Fn() -> Inner;
#[define_opaque(Outer)]
fn outer() -> Outer { || inner() }

fn main() { let _ = outer(); }
```

Run `rustc +nightly --edition=2024 minimal.rs`.

Actual result: E0282 at the closure, with the note
`cannot infer type of hidden type of opaque`. Adding an explicit `-> Inner`
return annotation to the closure does not resolve the error; the diagnostic
still suggests an explicit return type. Moving Inner and its constructor into
a separate module also fails. Both the default solver and
`-Znext-solver=globally` reject the minimal reproduction.

Expected result: the closure should use Inner as an already-defined opaque
type and define only Outer. If this is intentionally unsupported, a diagnostic
explaining that restriction would be helpful.

## Controls and impact

Changing only the outer function to RPIT compiles:

```rust
fn outer() -> impl Fn() -> Inner { || inner() }
```

Keeping the outer TAIT, but hiding Inner behind a nominal field, also compiles:

```rust
struct Value { value: Inner }
type Outer = impl Fn() -> Value;
#[define_opaque(Outer)]
fn outer() -> Outer { || Value { value: inner() } }
```

The analogous Coroutine `Return = Inner` case also fails. This affects
statically named coroutine operations that return or yield another operation.
A library workaround uses nominal suspension/completion wrappers, with no
boxing, to keep nested opaque types out of the outer TAIT's associated type
constraints. This observation narrows the trigger; it does not identify the
exact failing compiler query or establish that all nested TAIT bounds fail.

## Toolchain

```text
rustc 1.100.0-nightly (feaadeeac 2026-09-19)
binary: rustc
commit-hash: feaadeeaca7db0594da854e7c8c07495341c7439
commit-date: 2026-09-19
host: x86_64-unknown-linux-gnu
release: 1.100.0-nightly
LLVM version: 23.1.1
```

No regression range has been established.

## Related reports checked

- [TAIT tracking issue #63063](https://github.com/rust-lang/rust/issues/63063).
- [TAIT defining-use decision #117861](https://github.com/rust-lang/rust/issues/117861).
- [Projection ambiguity with a nested item bound, trait-system-refactor-initiative #69](https://github.com/rust-lang/trait-system-refactor-initiative/issues/69).

These are related background, not confirmed duplicates of this reproduction.

## Local reproduction files

[minimal.rs](minimal.rs), [coroutine.rs](coroutine.rs),
[nominal.rs](nominal.rs), and [rpit.rs](rpit.rs) are checked by
[verify.py](verify.py). From the repository root:

```sh
python3 experiments/tait-associated-output/verify.py
```

The runner retains diagnostics in a temporary directory and checks all four
cases with both solvers. The expected failures describe the observed compiler
limitation, not a desired restriction on library users.

# TAIT with Fn::Output equal to another TAIT fails with E0282; RPIT and nominal wrapper work

Published: [163088](https://github.com/rust-lang/rust/issues/163088). Submitted using the upstream [Bug Report template](https://github.com/rust-lang/rust/blob/main/.github/ISSUE_TEMPLATE/bug_report.md).

A TAIT with an associated output equal to another, separately defined TAIT is rejected with E0282. The analogous outer RPIT and a nominal output wrapper compile. This can be reproduced without dependencies, procedural macros, async code, or lifetimes.

I tried this code:

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

Command: `rustc +nightly --edition=2024 minimal.rs`.

I expected to see this happen: the closure uses `Inner` as an already-defined opaque type, while `outer` defines only `Outer`, so the program compiles. If this is intentionally unsupported, a diagnostic explaining that restriction would be helpful.

Instead, this happened:

```text
error[E0282]: type annotations needed
 --> minimal.rs:9:23
  |
9 | fn outer() -> Outer { || inner() }
  |                       ^^
  |
  = note: cannot infer type of hidden type of opaque
help: try giving this closure an explicit return type
  |
9 | fn outer() -> Outer { || -> /* Type */ { inner() } }
  |                          +++++++++++++++         +

error: aborting due to 1 previous error

For more information about this error, try `rustc --explain E0282`.
```

Adding `-> Inner` to the closure does not resolve the error (the diagnostic still suggests an explicit return type). Moving `Inner` and its constructor into a separate module also fails. The minimal example is rejected with both the default solver and `-Znext-solver=globally`.

### Passing controls

Keeping `Inner` and `inner` unchanged, replacing the outer TAIT and its defining function with RPIT compiles under both solvers:

```rust
fn outer() -> impl Fn() -> Inner { || inner() }
```

Keeping an outer TAIT but putting `Inner` behind a nominal field also compiles under both solvers:

```rust
struct Value { value: Inner }
type Outer = impl Fn() -> Value;
#[define_opaque(Outer)]
fn outer() -> Outer { || Value { value: inner() } }
```

### Coroutine reproduction

The analogous `Coroutine::Return` constraint also fails under both solvers:

```rust
#![feature(type_alias_impl_trait, coroutines, coroutine_trait)]
use std::ops::Coroutine;
type Inner = impl Coroutine<(), Yield=(), Return=usize>;
#[define_opaque(Inner)] fn inner() -> Inner { #[coroutine] static |_: ()| { if false {yield;} 8 } }
type Outer = impl Coroutine<(), Yield=(), Return=Inner>;
#[define_opaque(Outer)] fn outer() -> Outer { #[coroutine] static |_: ()| { if false {yield;} inner() } }
fn main() {let _ = outer();}
```

This affects named coroutine operations that return another operation. A library-level workaround uses nominal suspension/completion wrappers to avoid exposing nested opaque types directly in the outer TAIT's associated type constraints, without boxing. The repro above does not depend on that library.

### Meta

`rustc --version --verbose` (nightly):

```text
rustc 1.100.0-nightly (feaadeeac 2026-09-19)
binary: rustc
commit-hash: feaadeeaca7db0594da854e7c8c07495341c7439
commit-date: 2026-09-19
host: x86_64-unknown-linux-gnu
release: 1.100.0-nightly
LLVM version: 23.1.1
```

No regression range has been established. This is a normal compilation error, not an ICE; there is no panic backtrace.

Related background I checked: #63063, #117861, and https://github.com/rust-lang/trait-system-refactor-initiative/issues/69. I have not confirmed that any of them tracks this exact reproduction.

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

# nitori_sync

State that several owners reach one operation at a time, without the
library naming a lock. It is experimental and builds on stable Rust; the
repository still pins a nightly toolchain for its other crates. It needs
`alloc`, and the standard library only for its `std` feature.

A library that keeps a small table shared by many users, such as a store of
session tickets that every connection of a client reads and fills, needs
interior mutability, but which kind is the host's choice: a cell for one
that stays on one thread, a mutex for one whose users are on several. This
crate gives the two halves of that choice:

- `SharedState` runs one synchronous operation on the state, borrowed for as
  long as the operation runs. `RefCell<T>` implements it, and, with the `std`
  feature, `std::sync::Mutex<T>`; references, `Rc` and `Arc` forward it, so
  `&RefCell<T>`, `&mut RefCell<T>`, `Rc<RefCell<T>>`, `&Mutex<T>`,
  `Rc<Mutex<T>>` and `Arc<Mutex<T>>` all do. Through `&mut`, `with_mut`
  reaches the state without a runtime check.
- `Synchronization` is a family that the host chooses once: it names the
  shared wrapper of every kind of state and builds one with `share`.
  `RefCellSynchronization` wraps in `Rc<RefCell<T>>`; with the `std` feature,
  `MutexSynchronization` wraps in `Arc<Mutex<T>>`.

```rust
use nitori_sync::{RefCellSynchronization, SharedState, Synchronization};

fn counter<Y: Synchronization>() -> Y::SharedState<u32> {
    Y::share(0)
}

let shared = counter::<RefCellSynchronization>();
let other = shared.clone();
other.with(|count| *count += 1);
assert_eq!(shared.with(|count| *count), 1);
```

An operation must not borrow the same state again while it runs: a cell
panics and a mutex deadlocks. Nothing waits for the state, so an operation
never runs across an await. A mutex that a panic poisoned is taken as it is;
an operation that must leave the state consistent when it panics does so
itself.

## Features

- `std` (default): `SharedState` for `std::sync::Mutex` and the
  `MutexSynchronization` family. Without it the crate is `no_std`.

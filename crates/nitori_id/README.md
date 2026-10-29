# nitori_id

Identities unique within one process. It is experimental and builds on stable
Rust; the repository still pins a nightly toolchain for its other crates.

Code that refers to an object without holding it, such as a delayed
notification, a map key or a record of where something came from, needs an
identity that no other object in the process shares, even one of the same
kind created later. This crate provides two such values:

- `UniqueId` is an opaque value that no other `UniqueId::allocate` call in the
  process returns.
- `Identity` pairs the `TypeId` of a type, normally the concrete type that owns
  the identity, with a new `UniqueId`. `Identity::owner_type_id` returns that
  `TypeId`.

```rust
use nitori_id::Identity;
use std::any::TypeId;

struct Connection;

let first = Identity::new::<Connection>();
let second = Identity::new::<Connection>();

assert_ne!(first, second);
assert_eq!(first, first.clone());
assert_eq!(first.owner_type_id(), TypeId::of::<Connection>());
```

## Identity

`UniqueId` values come from one counter for the whole process, shared by all
types. They are never reused, so an identity kept after its object is gone, in
a clone, a pending event or a map key, can never be mistaken for a later
object. Allocation panics rather than repeat a value once the process has made
2^64 allocations, which at one per nanosecond would take more than 580 years.

`Eq`, `Hash` and `Ord` of an `Identity` compare its `TypeId` and `UniqueId`
and nothing else, so they agree with one another. The order is total and stays
the same within a process, but its meaning is unspecified. The type parameter
of `Identity::new` only labels the identity: it needs no value of the type and
imposes no bound other than `'static`, so unsized types work too.

An identity says nothing about its object. It neither keeps the object alive
nor implies that it still is, and it cannot be used to look the object up.

## Within one process only

Identities are meaningful only in the process that allocated them. Neither
type exposes its number, implements serialization, or can be rebuilt from an
external value, so an identity cannot be sent to another process or stored and
read back. `Debug` prints the type tag and number for diagnostics only; its
format is not stable.

Both types are `Send` and `Sync` and cheap to clone. They are deliberately not
`Copy`, so that their representation can later grow to carry more
information.

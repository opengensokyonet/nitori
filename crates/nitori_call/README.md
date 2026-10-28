# nitori_call

`nitori_call` runs operations written as ordinary async functions on a
resource that their caller lends for one poll at a time. An operation can await
other operations and ordinary futures, yield items, and return a final value.
It is experimental and requires the repository's pinned Rust nightly.

## Why calls?

Consider an outgoing message that must wait for permission before using a shared
connection. With an ordinary async function, acquiring the connection first
holds it throughout that unrelated wait:

```rust
use std::future::Future;
use tokio::sync::Mutex;

async fn send(shared: &Mutex<Vec<u8>>, permission: impl Future<Output = ()>) {
    let mut output = shared.lock().await;
    permission.await; // Other writers cannot use output during this wait.
    output.extend_from_slice(b"PING\n");
}
```

Moving the lock below the wait solves this particular case, but a leaf future
can only choose between holding the resource across waits and acquiring it on
every poll; several leaves completing in one poll then acquire it several times.
Only the code that polls the composed operation knows where a poll begins and
ends. A call therefore receives the resource from its caller on each poll, and
the caller decides how long it stays acquired:

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{calls::WriteAllError, prelude::*};
use std::future::Future;

#[call(sync)]
async fn send<F: Write, A: ReceiverAccess<Family = F>, Permission: Future<Output = ()>>(
    io: &mut A,
    permission: Permission,
    message: &[u8],
) -> Result<(), WriteAllError<F::Error>> {
    permission.await;
    io.write_all(message).await
}
# fn main() {}
```

`permission.await` uses no resource. An executor that acquires views on demand
(see [Executors](#executors)) therefore locks nothing until `write_all` reads
or writes. Ordinary async remains simpler when the operation owns its resource
for its whole duration.

## Read a length-prefixed record

The following operation reads a one-byte length and appends exactly that
payload. It uses the `nitori_io` capabilities, so the same code works with a
memory slice or an asynchronous IO bridge.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{calls::ReadExactError, prelude::*};

#[call(sync)]
async fn read_record<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    output: &mut Vec<u8>,
) -> Result<usize, ReadExactError<F::Error>> {
    let [length] = io.read_array::<1>().await?;
    io.read_exact(output, usize::from(length)).await?;
    Ok(usize::from(length))
}

fn main() {
    let mut input = b"\x03cat\x03dog".as_slice();
    let mut output = Vec::new();
    assert_eq!(input.sync_read_record_unpin(&mut output).unwrap(), 3);
    assert_eq!(output, b"cat");
    assert_eq!(input, b"\x03dog"); // The next record has not been read.
}
```

The first parameter is the access, `&mut A` with `A: ReceiverAccess<Family = F>`
or `&mut impl ReceiverAccess<Family = F>`, where the type parameter `F` is
bounded by the capabilities the operation needs; the operation is defined for
every family with those bounds. The output and the items can then write an
associated type of a capability as `F::Error`; arguments and where clauses
name it with its trait, as `<F as Write>::Error`. The access can also bound
its family directly, as `ReceiverAccess<Family: Write>`, and write
`<A::Family as Write>::Error`. Arguments, outputs and items may mention the
access only through its family. Crates that define operations enable
`impl_trait_in_assoc_type`.

`#[call]` keeps the function and generates:

- the operation struct `ReadRecord`, named after the function, with the other
  arguments as public fields and a `new` constructor; an argument written as a
  pattern, such as `(start, end): (u8, u8)`, is destructured by the function
  and its field is named `argument_N` after its position, counting the access
  as 0. When an argument's type mentions the family, the struct takes the
  family as a last type parameter, named `F` as in the function, or `Family`
  when the access bounds its family directly, and is an operation of that
  family only. Built directly rather than through the generated methods, it needs
  the family named, as in `Call::<F, _>::new(..)`;
- `ReadRecordExt`, with the method `io.read_record(..)` on receiver accesses
  inside other operations;
- `ReadRecordHostExt`, with `read_record` on a pinned executor and
  `read_record_unpin` on `&mut` to one that is `Unpin`. With `sync`, also
  `sync_read_record` and `sync_read_record_unpin`, which return the final value
  and panic if the operation waits.

Import the generated traits like other extension traits. `module = name` puts
the struct in a new module `name`. The function can also be called directly,
as `read_record(io, output)`, from another operation.

Bodies are ordinary async Rust: awaits, `join!`, `select!` and other macros
work as usual. An operation that needs a value from its caller once takes it as
an argument; a one-off root operation is an async function with `#[call]`
declared inside a block, whose generated items stay in that block.

## Receivers and families

A `ReceiverFamily` names the type of view lent for one poll,
`ReceiverView<'view>`. Views may own guards, be invariant or `!Unpin`; neither
views nor resources need `Send` or `Sync`. Capability traits, such as
`nitori_io::Read`, are implemented on families and take a pinned view.

A `Receiver` is a resource that constructs its view synchronously. `Direct<T>`
is the family whose view, `DirectView<'view, T>`, holds `Pin<&'view mut T>`;
standard containers implement `Receiver` with it, and the `bytes` feature adds
`Bytes` and `BytesMut`. `direct_receiver!(impl [generics] for Local)` associates
a local type with `Direct<Local>`; `family_receiver!(impl [generics] for Local)`
makes the local type its own family, with `BorrowedReceiver<'view, Local>` as
its view, so a downstream crate can implement a capability on it.

## Capabilities

`#[capability]` on a capability trait turns each `poll_name` method into a
primitive operation `Name`, which is also its future, and generates `{Trait}Ext`
and `{Trait}HostExt` the same way `#[call]` does. The structs go into a module
named after the trait in snake case, `module = name` or `module = self` choose
another. Type and const parameters of the trait become parameters of the
structs and of the traits; lifetime parameters are not supported. A poll method
whose arguments mention `Self`, such as `item: &Self::Item`, gives its struct a
`Family` parameter, as with `#[call]`.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverFamily, capability};
use std::{pin::Pin, task::{Context, Poll}};

#[capability(sync)]
pub trait Fill: ReceiverFamily {
    fn poll_fill<'view>(
        view: Pin<&mut Self::ReceiverView<'view>>,
        cx: &mut Context<'_>,
        value: u8,
    ) -> Poll<usize>
    where
        Self: 'view;
}
# fn main() {}
```

Associated types of a supertrait are written `<Self as Supertrait>::Name` in
the poll methods.

## Items and the final value

`#[call(yields = Item)]` makes each `yield value` written directly in the body
emit an item; the return value is the final value. `yield` inside closures
(including coroutines), async blocks, nested items and macro arguments keeps its
own meaning. The defining crate also enables `yield_expr`.

Every execution is both a `Future` of the final value and a `Stream` of
`ControlFlow` events: `Continue(item)` for each item, then `Break(final)`,
then the end. Awaiting it drops each item where it is emitted, without waiting
or waking. An execution may switch between the two between polls.

Inside an operation, the method of an emitting operation returns a `Child`,
which is the same pair of future and stream on the parent's access; await it to
keep only the final value, or pin it and call `next_event` to handle the
items. `next_event` gives each item, then the final value; as a `Stream`,
through `next`, the child also ends with `None` afterwards:

```rust
#![feature(impl_trait_in_assoc_type, yield_expr)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{calls::ReadChunksError, prelude::*};
use std::{num::NonZeroUsize, ops::ControlFlow, pin::pin};

#[call(yields = usize)]
async fn count_chunks<F: ReadChunk, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    length: usize,
) -> Result<usize, ReadChunksError<F::Error>> {
    let mut chunks = pin!(io.read_chunks(length, NonZeroUsize::new(4).unwrap()));
    let mut count = 0;
    loop {
        match chunks.as_mut().next_event().await {
            ControlFlow::Continue(chunk) => {
                count += 1;
                yield bytes::Buf::remaining(&chunk);
            }
            ControlFlow::Break(result) => return result.map(|_| count),
        }
    }
}
# fn main() {}
```

The [DATA parser](../../examples/data-frame-codec/src/current_codec.rs) yields
bounded payload chunks this way, and the
[write example](examples/call_composition.rs) reports progress before its final
byte count:

```sh
cargo run --locked -p nitori_call --example call_composition
cargo run --locked --example macro_demo
cargo run --locked -p nitori_io --example limited_read
```

Crates that only store, await or stream executions need no features.

## Composition: keep a child parser inside its record

Suppose a record contains a one-byte length followed by a two-byte word. If a
malformed record declares only one payload byte, an unrestricted word parser
would consume the next record's length as its second byte. Instead, adapt the
access with a byte budget: `nitori_io::adapters::ReadLimit` implements
`Compose<F>` and produces a view that still implements `nitori_io::Read` but
returns EOF at the record boundary. Its
[implementation](../nitori_io/src/adapters.rs) shows how to write such an
adapter; the [complete example](../nitori_io/examples/limited_read/main.rs)
also tests it across Pending and cancellation:

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{adapters::ReadLimit, calls::ReadArrayError, prelude::*};

#[call(sync)]
async fn read_word<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Result<[u8; 2], ReadArrayError<F::Error>> {
    io.read_array::<2>().await
}

#[call(sync)]
async fn read_record<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Result<[u8; 2], ReadArrayError<F::Error>> {
    let [length] = io.read_array::<1>().await?;
    io.compose(ReadLimit(usize::from(length))).read_word().await
}

fn main() {
    // Without the boundary, a two-byte parser consumes the next record's length.
    let mut payload = b"A\x02BC".as_slice();
    assert_eq!(payload.sync_read_word_unpin().unwrap(), *b"A\x02");

    // The malformed first record has only one byte; the next record stays readable.
    let mut input = b"\x01A\x02BC".as_slice();
    let error = input.sync_read_record_unpin().unwrap_err();
    assert_eq!(error.completed(), 1);
    assert_eq!(input, b"\x02BC");
    assert_eq!(input.sync_read_record_unpin().unwrap(), *b"BC");
}
```

`io.compose(state)` returns a `Composed` access that borrows its parent
mutably; on each access it builds the child view from the parent view and the
state. The state must be `Unpin`; pass `pin!(state)` otherwise. `into_state`
recovers it and ends the borrow. Generic adapters can keep the parent view as a
`ViewLoan<'view, F>`, which hides its inner lifetime: `loan.with(|access| ..)`
lends it back for one capability call.

## Receiver access

`ReceiverAccess` is implemented by the slot a root operation receives, by
`Composed`, and by `&mut A`; downstream crates do not implement it. Each use
borrows the access mutably, so two operations cannot use it at the same time,
and it never leaves the operation or the polling thread. Hand-written leaf
operations call `poll_access`, which lends the current view to a callback:

```rust
use nitori_call::{Direct, ReceiverAccess};
use std::{future::poll_fn, task::Poll};

async fn increment(io: &mut impl ReceiverAccess<Family = Direct<usize>>) -> usize {
    poll_fn(|cx| {
        io.poll_access(cx, |access, _| {
            let mut value = access.into_pin().get_pin_mut();
            *value += 1;
            Poll::Ready(*value)
        })
    })
    .await
}
# fn main() {}
```

When the call is polled without a view, the first access records a request
and the poll ends with `CallPoll::NeedsView`; polls that touch no resource need
no view.

## Calls

`Call<F, Op>` holds an operation and is polled with an optional view, lent as
`Lend::Fixed` or `Lend::Releasable` (see release points below): `poll_with`
delivers each event, `poll_return_with` only the final value. Its
type can be named, so a call can be a field of a connection that lends another
of its fields on each poll; successive polls may lend views of different
resources. A call is `Send` when everything its operation keeps apart from the
access is `Send`, even if the resource is not. `DynCall` is its dyn-compatible
form. `is_terminated` tells whether the call has delivered its final value.

## Executors

`CallExecutor` decides how an operation runs on a value. Every `Receiver` is an
executor whose execution, `BoundCall`, lends a fresh view on each poll; this is
what the generated host methods use for memory buffers and bridges.

An executor that can only acquire views, such as a reader of a connection behind
`tokio::sync::Mutex`, returns `drive(acquisition, operation)`. `Drive` polls the
operation without a view and acquires one only when the operation asks for it,
through a `ViewAcquisition` such as `AcquisitionState`, which keeps a pending
acquisition across polls. The view stays acquired until the end of that poll.
Waiting for anything else holds nothing: on polls where the operation does not
ask, `Drive` only advances a pending acquisition with `advance_unrequested`,
and releases a view granted meanwhile at once. The
[driver tests](tests/driver.rs) implement such a reader, and the
[executor tests](tests/executor.rs) lend one acquired view to several calls in
one pass.

Each poll of an execution is separate, so reading one as a stream releases the
view after every item: the reader may wait for something else before it asks
for the next one. To let the items that are ready in one poll share an
acquisition, read them inside an operation, which keeps the view for its whole
poll:

```rust
#![feature(impl_trait_in_assoc_type, yield_expr)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{calls::ReadExactError, prelude::*};
use std::{ops::ControlFlow, pin::pin};

/// Length-prefixed records, until an empty one.
#[call(yields = Vec<u8>)]
async fn read_records<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Result<(), ReadExactError<F::Error>> {
    loop {
        let [length] = io.read_array::<1>().await?;
        if length == 0 {
            return Ok(());
        }
        let mut record = Vec::new();
        io.read_exact(&mut record, usize::from(length)).await?;
        yield record;
    }
}

#[call(sync)]
async fn collect_records<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    records: &mut Vec<Vec<u8>>,
) -> Result<(), ReadExactError<F::Error>> {
    let mut items = pin!(io.read_records());
    loop {
        match items.as_mut().next_event().await {
            ControlFlow::Continue(record) => records.push(record),
            ControlFlow::Break(result) => return result,
        }
    }
}

fn main() {
    let mut input = b"\x01a\x02bc\x00".as_slice();
    let mut records = Vec::new();
    input.sync_collect_records_unpin(&mut records).unwrap();
    assert_eq!(records, [b"a".to_vec(), b"bc".to_vec()]);
}
```

Within one poll, an operation that alternates IO with computation also holds
the view while it computes. `io.release_view().await` marks a release point:
what follows does not need the view for a while. Whether releasing pays off
depends on the resource, so both sides opt in. An acquisition that delivers
`Lend::Releasable` views, such as `AcquisitionState::releasable`, lets `Drive`
release the view there, and `ViewAcquisition::should_release` decides at each
release point, for example only while others wait for the lock. `Drive` then
continues the operation at once without the view and acquires a new one at the
next access; the task is not suspended, so others can take the resource while
the operation computes on another thread. With a fixed view, as from every
`Receiver`, or without one, a release point completes at once.

`drive_composed(acquisition, state, operation)` composes each acquired view
with a `Compose` state before lending it, so the operation runs on the composed
family; the composed view borrows the acquired one while it is lent, and is
releasable when the acquired view is. `drive` lends the acquired views as they
are.

Locks are executors built this way. `Lock` describes a lock whose guard is
acquired by a future, `Locked<L>` is the family whose view is that guard, and
`LockAcquisition` locks on demand and delivers releasable guards. `Guarded`
composes a guard into the view of the value behind it, so `&Mutex<T>` runs
operations on `T`'s own family, with all of its capabilities, locking only on
polls that use `T` and releasing the lock at the end of each and at release
points:

```rust
use nitori_io::prelude::*;
use std::sync::Mutex;

fn main() {
    let output = Mutex::new(Vec::new());
    (&output).sync_write_all_unpin(&b"PING"[..]).unwrap();
    assert_eq!(*output.lock().unwrap(), b"PING");
}
```

`&std::sync::Mutex<T>` is an executor whenever `T` is an `Unpin` receiver;
it blocks the polling thread while another thread holds the lock and panics
when the lock is poisoned. The `tokio-sync` feature adds
`&tokio::sync::Mutex<T>`, which needs nightly for its lock future, and the
`futures-lock` feature adds `&futures::lock::Mutex<T>`. Other lock types
implement `Lock` and use `drive_composed` in an executor of their own. A state
other than `Guarded` reaches a part of the locked value, such as one stream of
a connection; when that part is gone, its view reports it, as in the
[lock tests](tests/locks.rs).

An operation that needs two resources behind separate locks has two choices.
A family whose view holds both guards acquires them together, in a fixed
order, and lends both for the same polls. Or the operation runs a nested
`drive(acquisition, operation)` for the second resource, which acquires it
only for the polls that use it; the first resource is not held while the
nested operation waits. The [driver tests](tests/driver.rs) show both.

`BoundCall`, `Drive` and `Child` implement `FusedFuture` and `FusedStream`, as
`futures::select!` requires. Dropping a call cancels it without rolling back
completed IO.

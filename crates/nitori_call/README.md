# nitori_call

`nitori_call` describes resumable operations that use a resource, await other
operations or ordinary futures, and optionally yield values before returning.
It is experimental and requires the repository's pinned Rust nightly. Start here
for usage; the sections below cover the execution and borrowing contracts.

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

Moving the lock below the wait solves this particular case. Calls are useful
when that access policy belongs to a driver and the protocol is reused across
memory buffers, sockets, and acquired views. The protocol can await permission
and request IO without owning the lock or borrowing a connection at construction:

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Direct, Target, run_sync};
use nitori_io::{Write, TargetWriteExt, calls::WriteAllError};
use std::{future::{Future, ready}, pin::Pin};

#[call(sync)]
async fn send<'data, H: Write, Permission: Future<Output = ()>>(
    io: Target<H>, permission: Permission, message: &'data [u8],
) -> Result<usize, WriteAllError<H::Error>> {
    permission.await;
    io.write_all(message).await.result
}

fn main() {
    // Prepare work before selecting or borrowing a particular output buffer.
    let operation = send::<Direct<Vec<u8>>, _>(ready(()), b"PING\n");
    let mut output = Vec::new();
    output.extend_from_slice(b"HELLO\n");
    assert_eq!(run_sync(Pin::new(&mut output), operation).unwrap(), 5);
    assert_eq!(output, b"HELLO\nPING\n");
}
```

`permission.await` needs no receiver access. A lazy acquisition driver therefore
need not lock anything until `write_all` requests IO. The
[async Mutex driver tests](tests/resource_driver.rs) implement that policy.
The ordinary bound entry point, `output.send_unpin(permission, message).await`,
retains its resource borrow until dropped: binding a call does not automatically
turn that borrow into a lock-per-poll policy. Ordinary async remains simpler
when one borrowed resource for the whole operation is the desired contract.

## Read a length-prefixed record

The following parser reads a one-byte length and appends exactly that payload.
It uses `nitori_io` capabilities directly, so the same code works with a memory
slice or an asynchronous IO bridge. Neither the record format nor its error
handling needs a separate polling state machine.

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Target};
use nitori_io::{Read, TargetReadExt, calls::ReadExactError};

#[call(sync)]
async fn read_record<'data, H: Read>(
    io: Target<H>, output: &'data mut Vec<u8>,
) -> Result<usize, ReadExactError<H::Error>> {
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

`#[call(sync)]` adds a synchronous entry point alongside the asynchronous one.
Use it only for immediately ready resources: it panics on `Pending`. For a
receiver that may wait, use `input.read_record_unpin(&mut output).await` inside
an async function. `_unpin` accepts `&mut H`; pinned resources use
`input.as_mut().read_record(&mut output)`. Import the generated `ReadRecordExt`
when the call is declared in another module.

For large payloads, collecting into a Vec is often undesirable. A call can
instead declare `yields = H::Chunk` and yield one bounded chunk at a time,
then return a separate success or error. The
[DATA parser](../../examples/data-frame-codec/src/current_codec.rs) does this
using `read_chunks_exact`; the [consumer](../../examples/data-frame-codec/examples/macro_demo/main.rs)
processes chunks without buffering a whole frame. Awaiting a yielding call
ignores its yielded values; iterate its events when those values are needed.
The [write example](examples/call_composition.rs) similarly reports progress
before returning its final byte count.

Both crates are workspace dependencies in this repository and are not yet
published to crates.io. From the repository root:

```sh
cargo run --locked -p nitori_call --example call_composition
cargo run --locked --example macro_demo
cargo run --locked -p nitori_io --example limited_read
```

## Receivers and families

`ReceiverFamily::ReceiverView<'view>` implements `HasReceiverFamily<Family = Self>`.
Capability identity does not require view reconstruction. Owning guards, invariant
views and `!Unpin` views are supported without a `Send` or `Sync` requirement.
`Receiver` is the optional synchronous view-construction interface for already
accessible resources. Its implementations also have `HasReceiverFamily`.

A scope lazily acquires and pins one view. `poll_view` repeatedly lends that same
value; `poll_ready` is a readiness-only convenience with sticky Ready. The outer
borrow lifetime and the view's internal resource lifetime are distinct. View
construction may have a cost and runs once per scope, after obtaining access.

For simple resources, `Direct<T>` uses `DirectView<'visit, T>`, containing a
`Pin<&'visit mut T>` in its public `.0` field. Standard containers and scalar
hosts have canonical associations; the optional `bytes` feature adds Bytes and
BytesMut. `direct_receiver!(impl [generics] for LocalType)` associates a local type
with `Direct<LocalType>`.

Alternatively, `family_receiver!(impl [generics] for LocalType)` uses the local type
itself as the family marker and `BorrowedReceiver<'visit, LocalType>` as its view.
This allows a downstream crate to implement an external capability trait on
its local family without violating Rust's orphan rules. Both helpers are
optional: custom composition views implement `HasReceiverFamily`; their families
implement `ReceiverFamily`. They do not need to implement `Receiver`.
Unsized resources can be accessed through a sized `DirectView`.

## Targets and extension methods

The first `#[call]` parameter is `Target<F>`. It is a real, copyable, host-free
value. It may be moved, stored, renamed, captured by a closure, or returned.
`call_closure!(|io: Target<F>| { ... })` defines an anonymous operation;
an omitted receiver annotation can be inferred from the execution context.

A named call generates its operation, constructor, `NameArguments`, and two
extension traits:

- `NameExt<...>` applies to every `H: ResourceDriver` whose family satisfies the
  declaration. It provides `name` on `Pin<&mut H>` and `name_unpin` on `&mut H`
  when `H: Unpin`. Its result is the driver-selected `Execution` type.
- `TargetNameExt<...>` applies to any `CallTarget` whose target family satisfies
  the declaration, including root receivers and composed receivers. Its method
  consumes that receiver and constructs a host-free child.

The traits must be in scope, just like other Rust extension traits. Family
parameters remain in the generated trait's generic parameters. Arguments whose
types depend on the family retain those parameters too.

`Arguments<F>::into_call` selects and constructs an operation. A receiver's
`call(arguments)` uses that protocol; `operation(operation)` accepts an already
constructed `CallOn<Self::Target>`. The macro does not identify methods by name
or track receiver variables. It lowers awaits and yields, validates the syntax
boundary, and hides the current-resume environment.

## Composition: keep a child parser inside its record

Suppose a record contains a one-byte length followed by a two-byte word. If a
malformed record declares only one payload byte, calling an unrestricted word
parser would consume the next record's length as its second byte. A length
check inside every child parser would couple each parser to its caller's framing.

Instead, compose the receiver with a byte budget. `ReadLimit` implements
`Compose<F>` and produces a view that still implements `nitori_io::Read`, but
returns EOF at the record boundary. The child stays generic over `Read`.
Here is the [complete example](../nitori_io/examples/limited_read/main.rs), using
the [ReadLimit adapter](../nitori_io/examples/limited_read/limit.rs):

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

# mod limit { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../nitori_io/examples/limited_read/limit.rs")); }

use limit::ReadLimit;
use nitori_call::{Target, TargetExt, call};
use nitori_io::{Read, TargetReadExt, calls::ReadArrayError};

// This parser can be reused with either an unrestricted or a limited receiver.
#[call(sync)]
async fn read_word<H: Read>(io: Target<H>) -> Result<[u8; 2], ReadArrayError<H::Error>> {
    io.read_array::<2>().await
}

#[call(sync)]
async fn read_record<H: Read>(io: Target<H>) -> Result<[u8; 2], ReadArrayError<H::Error>> {
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

The state has a concrete job: `ReadLimit.0` counts bytes still available to the child. Successful reads
reduce it; `Pending` and errors leave it unchanged; zero returns EOF without
polling the parent. It survives child suspension while each temporary view only
borrows it. The example's tests check partial reads, Pending, and cancellation.
Dropping a call preserves unread bytes but does not restore bytes already read.
For a longer declared record, the caller must explicitly consume or skip any
remaining payload before starting the next record.

Only the adapter defines a new family/view; protocol code uses the existing
`Read` capability. The same pattern can add checksums or byte accounting to
existing parsers without changing their signatures. It is a scoped adaptation
of the current receiver, not a second stream or a copy of its contents.

`io.compose(state)` returns a `Composed<R, State>` description. On demand,
`Compose::compose` borrows the current parent view and state to produce the
child view. Intermediate views stay pinned until that drive returns. The
child runs against the adapted family, while its parent keeps the original one.

A description may own its state, borrow `&mut S` when `S: Unpin`, or retain
`Pin<&mut S>` for pinned state. A child borrows the description through
`&mut composed` or `composed.as_mut()` after pinning. Those are ordinary Rust
borrows: state cannot be reused while the child still needs it. `into_parts`
recovers an unpinned description and its state. A pinned owned state cannot be
moved out through a pinned reference.

The [composition tests](../nitori_io/tests/composition.rs) demonstrate added
capabilities, nested child-local state, non-static input, invariant `!Unpin`
views, partial reads, and cancellation.

Opaque generic decorators can store `ViewLoan<'scope, F>` rather than an owned
parent view. `loan.with(|access| F::capability(access.into_pin(), ...))` preserves
static capability dispatch without a capability registry. The loan hides only
the parent view lifetime; F remains invariant and exact. A visit cannot return a
borrow into its temporary access. Typed field projections can use the original
`Compose` input directly instead.

## Receiver access and awaiting

`io.with(callback).await` is a host-aware request that calls the callback once
after the scope is ready, and then completes. The callback receives `Access<'access, 'host, F>`;
`into_pin()` exposes the current pinned view. This nominal argument supports
borrowed families without imposing an accidental `'static` bound. The output
cannot depend on the temporary access lifetime. References to independently
long-lived input data may be returned when the family's capability allows it.

All awaits use `IntoAwaitOn` / `AwaitOn`. Standard `IntoFuture` values receive
the current context. `Child` and `Next` receive the current receiver
scope. Directly awaiting a child discards intermediate yields and returns its
completion. Pin a child, then use `child.as_mut().next().await` to obtain
`Some(Yielded(item))`, a unique `Some(Complete(result))`, then `None`.
Children may themselves be returned or yielded and retain their real input and
state borrows. A standard future cannot poll a host-aware child without binding
a host or entering a call.

Only `pin!`, `std::pin::pin!`, and `core::pin::pin!` are intrinsic macros within
call bodies. Other opaque macros and attributes are rejected because they could
hide a suspension from the lowering pass. Nested ordinary async blocks and
closures retain their own suspension scope and may capture real receivers;
their host-aware values still require a call context to execute.

## Bound and synchronous execution

`BoundCall::new(pinned_host, operation)` retains an actual host borrow and
implements Future and Stream. Awaiting it discards yields; its stream and pinned
`next()` preserve events. Each poll creates a lazy scope; `Receiver::view` runs only if requested. `poll_receiver` from
`PollCallExt` performs one poll without retaining the resource borrow.

`#[call(sync)]` additionally generates `sync_name` and `sync_name_unpin` on
actual hosts. Without `yields`, these return completion directly. With a
`yields = Item` declaration, they return a lazy `SyncBoundCall`, including when
Item is Infallible. Pin that binding to iterate through yields and completion.
The same child-await syntax works inside synchronous and asynchronous parents.

`run_sync` executes a no-yield operation; `step_sync` advances one event;
`SyncBoundCall` provides fused event iteration. They use a no-op waker and panic
on Pending rather than blocking or retrying. A bound call terminates on panic.
Manual poll drivers own completion and panic tracking and must not resume a
completed or panicked operation. Successive polls must supply the logical
resources required by the operation.

Dropping a call cancels remaining work without rolling back completed IO.
Bindings retain their host borrow until dropped, even after completion. Calling
a synchronous host method inside `with` is permitted, but a binding borrowing
the temporary view must be consumed before the callback returns.

The [stream composition tests](tests/call_composition.rs) cover asynchronous
waiting, sequential results, child events, and cancellation without prefetching.

The runtime uses a stack-local dispatch slot per resume; it performs no per-poll
allocation or TLS lookup and never casts one lifetime-indexed host type to
another. Nominal coroutine output wrappers avoid a nightly inference limitation
with nested opaque output types.

## Resource drivers

`ResourceDriver::Execution<'driver, O>` implements both Future and Stream.
The driver selects its storage layout. `Receiver` implementations get a direct
borrowed driver through `BoundCall`; custom async drivers can implement the GAT.

An async driver can capture its resource in an ordinary `#[call]` with
`Target<ExecutionControl>`. `Execution::new(outer_call)` supplies Event control
when polled as a Stream and Return control when polled as a Future. The control
is an ordinary receiver; `ResumeContext` carries no execution-mode flag.

The outer call pins an `AcquisitionState` and its business operation, then loops
over `drive(acquisition.as_mut(), operation.as_mut()).await`, forwarding Yielded
and returning Complete. Each poll of `Drive` reads the current control, creates
an `AcquiredScope`, invokes the business operation and destroys the scope before
returning. Pending propagates through ordinary await lowering. Switching poll
modes after Pending does not recreate the awaitable or acquisition.

Event mode calls the complete business operation's `poll_call`. Return mode calls
its `poll_return`, consuming its yields within one scope. Event transformations
belong inside that business operation; the outer resource-driving call must only
forward its events, because Return mode may consume them before the outer call
sees them. No arbitrary nested call is bypassed.

`AcquisitionState` starts its future on demand and retains it across Pending,
ordinary-input waits and outward events. Ready transfers the complete view into
the current scope and destroys the completed future. Ending the scope drops its
view; a later access starts a new acquisition. Completion, cancellation and panic
must destroy outstanding acquisition state before the resource it borrows.
Custom `ViewAcquisition` implementations must release their pending state on Drop.

The [resource-driver tests](tests/resource_driver.rs) show owning and borrowed
async Mutex drivers written with the public macro, with non-static data and
pinned views. Compiler-generated call state stores the borrowing acquisition
future; no hand-written self-referential container or lifetime extension is needed.

# nitori_call

Host-parameterized operations with optional intermediate yields. `CallOn` keeps
operation state while each poll receives a short pinned host borrow. `BoundCall`
binds a host borrow for asynchronous execution: it implements Future and Stream.
Awaiting it discards intermediate yields; use its event stream to preserve them.

## Child calls and host access

The first `#[call]` parameter is a virtual `Receiver<'_, Host>`. A direct
`io.decode(args)` constructs a real `Child<Host, Decode>` without borrowing the
Host. It may be moved, stored, passed to ordinary functions, or captured by
closures before it is pinned. Direct `.await` discards child yields and returns
its final result. To observe events, explicitly pin the child and await
`child.as_mut().next()`: it returns `Some(Yielded(item))`, a unique
`Some(Complete(result))`, then `None`.

All awaits use `IntoAwaitOn` / `AwaitOn`. Standard IntoFuture values receive the
current Context; Child and Next additionally receive the current pinned Host.
Next is a host-aware awaitable, not a standard Future. The macro does not track
child variable declarations or recognize calls to `next` by their name.

Use `io.with(|host| ...)` for synchronous access to the actual pinned Host.
`io.as_ref().method()` and `io.as_mut().method()` are short-access conveniences;
their Host references cannot escape. Projection chains run inside that access,
so use `.with` for complex synchronous expressions and compute asynchronous
arguments before entering them. `io.sync_name(args)` also uses short access. A
yielding synchronous binding cannot escape this short access; consume it within
`.with`.

`pin!(expression)`, `std::pin::pin!(expression)` and `core::pin::pin!(expression)`
are reserved macro intrinsics expanded to `::core::pin::pin!`. Other opaque
macros are rejected. Nested ordinary async blocks and closures retain their own
scope and cannot capture the virtual receiver.

Outside the macro, `Receiver::from_pin` supports any pinned Host and
`Receiver::from_mut` supports Unpin hosts. The macro generates a separate
`ReceiverNameExt` trait with `name` and, when requested, `sync_name`, borrowing
`&mut Receiver`. The original Host extension trait and its four entries remain
available. Ordinary async code can use the real Receiver and bound futures; its
borrowing model differs from virtual macro access.

Legacy `Pin<&mut Host>` annotations remain accepted for source compatibility,
including their old synchronous direct-method syntax. Their child awaits use
the same type-directed protocol. New code should use Receiver.

## Higher-order operations

Calls may return or yield an owned child operation. The child retains its real
input borrows, so normal Rust lifetime rules apply when it is handed to another
consumer. Awaiting the factory and awaiting the returned child are separate
steps:

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Child, Receiver};

#[call]
async fn increment(io: Receiver<'_, usize>) -> usize {
    io.with(|mut host| { *host += 1; *host })
}
#[call]
async fn factory(io: Receiver<'_, usize>) -> Child<usize, Increment> {
    io.increment()
}
#[call(sync)]
async fn execute(io: Receiver<'_, usize>) -> usize {
    let child = io.factory().await;
    child.await
}
fn main() {
    let mut host = 0;
    assert_eq!(host.sync_execute_unpin(), 1);
}
```

The generated coroutine uses nominal internal Yield and Return wrappers to
avoid a compiler inference limitation involving nested opaque associated types.
This does not change the public CallOn types and introduces no heap allocation.
The standalone [compiler reproduction and upstream report](../../experiments/tait-associated-output/ISSUE.md)
record the limitation and controls.

## Synchronous methods

`#[call(sync)]` adds `sync_name` and `sync_name_unpin` to the generated extension
trait, alongside the existing `name` and `name_unpin` methods. The first receiver
is `Pin<&mut Host>`; the `_unpin` receiver is `&mut Host` with `Host: Unpin`.
Both execute the same generated operation as the asynchronous methods.

Without a `yields` declaration, synchronous methods immediately return the
operation's original return type. With `yields = Item`, they return a lazy
`SyncBoundCall`, even if the operation produces no items on a particular run or
explicitly declares `yields = Infallible`. Attribute order is unrestricted.

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Receiver};
use std::{ops::CoroutineState, pin::pin};

#[call(sync)]
async fn add(io: Receiver<'_, usize>, amount: usize) -> usize {
    io.with(|mut host| { *host += amount; *host })
}

#[call(sync, yields = usize)]
async fn steps(io: Receiver<'_, usize>, count: usize) -> usize {
    for _ in 0..count {
        yield io.add(1).await;
    }
    io.with(|host| *host)
}

fn main() {
let mut host = 0;
assert_eq!(host.sync_add_unpin(2), 2);
{
    let mut events = pin!(host.sync_steps_unpin(2));
    let observed: Vec<_> = events.as_mut().collect();
    assert_eq!(observed, vec![
        CoroutineState::Yielded(3),
        CoroutineState::Yielded(4),
        CoroutineState::Complete(4),
    ]);
    assert_eq!(events.as_mut().next(), None);
}
assert_eq!(host, 4);
}
```

A synchronous parent can consume a yielding child through the same
`pin!(io.child(args))` and `child.as_mut().next().await` body used by an
asynchronous parent. With `#[call(sync, yields = Item)]`, its generated sync
entry yields one parent event per iterator step. The parent may use `.with`
between child events; no child binding retains the Host borrow between steps.
The presence of `.await` does not require an executor when all polls are ready.

Alternatively, a `.with` callback may construct a real `host.sync_child(args)`
binding, pin it, and consume its iterator before returning owned results.
`let events = io.sync_child(args)` cannot retain that binding outside the short
access: the binding borrows the Host. This is rejected by the borrow checker.

This is a current limitation of explicit synchronous child composition inside
`#[call]`. The macro recognizes `sync_*`; the obstacle is that SyncBoundCall
combines persistent operation state with a persistent Host borrow, while the
virtual receiver grants only short access. Outside the macro, a real Host or
Receiver can supply the longer borrow and the binding can be retained.

Driving the whole parent synchronously does not express a separate requirement
that one particular child be advanced synchronously inside an otherwise
asynchronous parent. There is currently no generated host-free synchronous event
handle that automatically reacquires the Host on each step. Such an adapter
would be a possible future extension, not an implemented or committed API.

The operation can be `!Unpin` even when the host is `Unpin`. Pin the event binding
before advancing it; `Pin<&mut SyncBoundCall>` implements Iterator and
FusedIterator without requiring a heap allocation. Each step returns one yield
or the unique completion event, followed by `None`. Consumers must handle the
completion event to observe the final result, including any business error.
Construction does not advance the operation. Dropping a binding cancels remaining
work and releases its host borrow; previous reads and writes are not rolled back.
The binding retains its host borrow even after delivering completion until it is
released.

All dependencies must complete immediately in synchronous execution. Each step
uses a no-op waker and panics on `Pending`; it does not block, retry, or convert
waiting into an incomplete-input error. A SyncBoundCall terminates on panic, so
catching that panic and advancing again returns `None`. The `sync` attribute is
not a compile-time proof of readiness: ordinary futures and host implementations
can still violate the contract. Business errors remain in the original return
type and do not acquire an additional adapter error wrapper.

## Adapting other operations

`run_sync(pinned_host, operation)` pins and executes any
`CallOn<Host, Yield = Infallible>`, including manually implemented operations and
`call_closure` results. `SyncBoundCall::new(pinned_host, operation)` provides
synchronous event iteration for any CallOn.

`step_sync(pinned_operation, pinned_host)` advances one event without binding a
persistent host borrow. It supports callers that reacquire controlled access on
each step. Such callers own completion and panic tracking and must not resume an
operation after completion or panic. Successive accesses must refer to the same
logical host resources required by the operation.


## Sequential call composition

The runnable [call composition example](examples/call_composition.rs) uses the
existing macro and typed child protocol to implement two generic operations:

- `call_results`: accepts `Stream<Item = Operation>` where `Operation: CallOn<Host>`,
  executes one operation at a time, and yields each final `Operation::Return`.
  Intermediate child yields are discarded by the existing direct-await contract.
- `call_events`: accepts the same source and yields each child's
  `CoroutineState<Operation::Yield, Operation::Return>`, preserving completion
  boundaries. The outer operation returns `()` when the source ends.

`IterCalls` adapts an iterator without prefetching; `std::iter::from_fn` supplies
a closure factory, and `map` constructs operations from successive parameters.
The stream can be `!Unpin`, and operation values may retain input borrows. Each
source has one concrete operation type; heterogeneous operations require a
common representation and are not automatically erased. The input items are
unbound operations, not `BoundCall` or `Child` wrappers.

No next input is requested until the current call completes and its output is
consumed. The adapter does not control buffering inside a supplied source.
A child `Complete` is an outer `Yielded(Complete(...))`; only source exhaustion
produces outer `Complete(())`. Return values are opaque: an `Err` is delivered
as a value, and does not automatically stop later calls. Fallible sources and
stop-on-error policies require a separately specified adapter.

Both operations use `#[call(sync, yields = ...)]`, so the same bodies support
asynchronous event consumption and synchronous iteration. Synchronous execution
requires both the source and every child dependency to be immediately ready.
Use the event interface to consume their outputs: directly awaiting the whole
composition discards them and may never finish for an infinite source. A child
that yields forever also prevents the results-only variant from producing a
final result. Cancellation drops the current state without rollback.

Run from the repository root:

```sh
cargo +nightly run --locked -p nitori_call --example call_composition
cargo +nightly test --locked -p nitori_call --example call_composition
```

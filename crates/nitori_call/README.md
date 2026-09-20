# nitori_call

Host-parameterized operations with optional intermediate yields. `CallOn` keeps
operation state while each poll receives a short pinned host borrow. `BoundCall`
binds a host borrow for asynchronous execution: it implements Future and Stream.
Awaiting it discards intermediate yields; use its event stream to preserve them.

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
use nitori_call::call;
use std::{ops::CoroutineState, pin::{Pin, pin}};

#[call(sync)]
async fn add(io: Pin<&mut usize>, amount: usize) -> usize {
    io.with(|mut host| { *host += amount; *host })
}

#[call(sync, yields = usize)]
async fn steps(io: Pin<&mut usize>, count: usize) -> usize {
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

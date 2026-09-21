# nitori_call

Family-based operations with optional intermediate yields. An operation implements
`CallOn<F>` and retains its own state. Each poll receives a freshly borrowed
`Pin<&mut F::Host<'visit>>`; the inner lifetime can differ on every poll.

## Hosts and families

`Host::Family` selects the resource's canonical family. `Host::view` reconstructs
that family's view for the current access. `HostFamily::Host<'visit>` must itself
implement `Host<Family = Self>`, so a reconstructed view uses the same family.
The family is a type marker and need not exist as a runtime value.

Authors define their view types and their reconstruction logic. A view may own
another temporary view alongside a borrowed state. No covariance, lifetime
transmutation, `Send`, `Sync`, or `Unpin` requirement is imposed on those views.
The outer pinned borrow and the lifetime inside the view are separate.

For simple resources, `Direct<T>` uses `DirectView<'visit, T>`, containing a
`Pin<&'visit mut T>` in its public `.0` field. Standard containers and scalar
hosts have canonical associations; the optional `bytes` feature adds Bytes and
BytesMut. `direct_host!(impl [generics] for LocalType)` associates a local type
with `Direct<LocalType>`.

Alternatively, `family_host!(impl [generics] for LocalType)` uses the local type
itself as the family marker and `BorrowedHost<'visit, LocalType>` as its view.
This allows a downstream crate to implement an external capability trait on
its local family without violating Rust's orphan rules. Both helpers are
optional: custom composition views use explicit `Host` and `HostFamily` impls.
Unsized resources can be accessed through a sized `DirectView`.

## Real receivers and extension methods

The first `#[call]` parameter is `Receiver<F>`. It is a real, copyable, host-free
value. It may be moved, stored, renamed, captured by a closure, or returned.
`call_closure!(|io: Receiver<F>| { ... })` defines an anonymous operation;
an omitted receiver annotation can be inferred from the execution context.

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Direct, Receiver, ReceiverExt};

#[call(sync)]
async fn add(io: Receiver<Direct<usize>>, amount: usize) -> usize {
    io.with(|access| {
        let mut host = access.into_pin().get_mut().0.as_mut();
        *host += amount;
        *host
    }).await
}
#[call(sync)]
async fn twice(io: Receiver<Direct<usize>>, amount: usize) -> usize {
    let receiver = io;
    receiver.add(amount).await;
    receiver.add(amount).await
}
fn main() {
    let mut host = 0usize;
    assert_eq!(host.sync_twice_unpin(2), 4);
}
```

A named call generates its operation, constructor, `NameArguments`, and two
extension traits:

- `NameExt<...>` applies to every actual `H: Host` whose family satisfies the
  declaration. It provides `name` on `Pin<&mut H>` and `name_unpin` on `&mut H`
  when `H: Unpin`. Reconstructed host views also get these methods.
- `ReceiverNameExt<...>` applies to any `Route` whose target family satisfies
  the declaration, including root receivers and composed receivers. Its method
  consumes that route and constructs a host-free child.

The traits must be in scope, just like other Rust extension traits. Family
parameters remain in the generated trait's generic parameters. Arguments whose
types depend on the family retain those parameters too.

`Arguments<F>::into_call` selects and constructs an operation. A receiver's
`call(arguments)` uses that protocol; `operation(operation)` accepts an already
constructed `CallOn<Self::Target>`. The macro does not identify methods by name
or track receiver variables. It lowers awaits and yields, validates the syntax
boundary, and hides the current-resume environment.

## Composition

`io.compose(state)` returns a real `Composed<Route, State>` description. State
implements `Compose<InnerFamily>` and supplies an associated output family.
On each child poll the route reconstructs the inner view, then calls the state's
`compose` method to build the author-defined outer view. The child executes on
that output family; the routed operation itself executes on the original root.
Inside a child call, its receiver starts from that child's declared family.

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Compose, Direct, DirectView, Receiver, ReceiverExt};
use std::pin::Pin;

struct State(usize);
impl Compose<Direct<()>> for State {
    type Family = Direct<usize>;
    fn compose<'v>(self: Pin<&'v mut Self>, _: DirectView<'v, ()>)
        -> DirectView<'v, usize> where Direct<()>: 'v, Self::Family: 'v {
        DirectView(Pin::new(&mut self.get_mut().0))
    }
}
#[call]
async fn read(io: Receiver<Direct<usize>>) -> usize {
    io.with(|access| *access.into_pin().get_mut().0).await
}
#[call(sync)]
async fn parent(io: Receiver<Direct<()>>) -> usize {
    let mut state = State(7);
    let mut composed = io.compose(&mut state);
    let first = (&mut composed).read().await;
    let second = (&mut composed).call(ReadArguments::new()).await;
    let (_, state) = composed.into_parts();
    first + second + state.0
}
fn main() { assert_eq!(().sync_parent_unpin(), 21); }
```

A description may own its state, borrow `&mut S` when `S: Unpin`, or retain
`Pin<&mut S>` for pinned state. A child borrows the description through
`&mut composed` or `composed.as_mut()` after pinning. Those are ordinary Rust
borrows: state cannot be reused while the child still needs it. `into_parts`
recovers an unpinned description and its state. A pinned owned state cannot be
moved out through a pinned reference.

The [composition tests](../nitori_io/tests/composition.rs) demonstrate added
capabilities, nested child-local state, non-static input, invariant `!Unpin`
views, partial reads, and cancellation.

## Host access and awaiting

`io.with(callback).await` is a host-aware request that calls the callback once
and completes immediately. The callback receives `Access<'access, 'host, F>`;
`into_pin()` exposes the current pinned view. This nominal argument supports
borrowed families without imposing an accidental `'static` bound. The output
cannot depend on the temporary access lifetime. References to independently
long-lived input data may be returned when the family's capability allows it.

All awaits use `IntoAwaitOn` / `AwaitOn`. Standard `IntoFuture` values receive
the current context. `Child` and `Next` additionally receive the current host
view. Directly awaiting a child discards intermediate yields and returns its
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
`next()` preserve events. Each poll calls `Host::view` again. `poll_host` from
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

The [sequential composition example](examples/call_composition.rs) consumes an
iterator or Stream of operations without prefetching and either yields each
completion or relays every child event. It demonstrates asynchronous waiting,
synchronous iteration, and cancellation. Run it from the repository root with
`cargo +nightly run --locked -p nitori_call --example call_composition`.

The runtime uses a stack-local dispatch slot per resume; it performs no per-poll
allocation or TLS lookup and never casts one lifetime-indexed host type to
another. Nominal coroutine output wrappers avoid a nightly inference limitation
recorded in the [compiler reproduction](../../experiments/tait-associated-output/ISSUE.md).

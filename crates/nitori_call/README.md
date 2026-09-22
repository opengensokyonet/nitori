# nitori_call

Calls can suspend on ordinary futures, access receivers lazily, and yield intermediate
values before returning. `CallOn<F>` receives a `ReceiverScope`, not an eagerly
constructed view. Ordinary async waits do not acquire a receiver.

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

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Direct, Target, TargetExt};

#[call(sync)]
async fn add(io: Target<Direct<usize>>, amount: usize) -> usize {
    io.with(|access| {
        let mut host = access.into_pin().get_mut().0.as_mut();
        *host += amount;
        *host
    }).await
}
#[call(sync)]
async fn twice(io: Target<Direct<usize>>, amount: usize) -> usize {
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

## Composition

`io.compose(state)` returns a real `Composed<R, State>` description. State
implements `Compose<InnerFamily>` and supplies an associated output family.
Each child drive builds nested lazy scopes. On demand, `compose` borrows the
existing parent view and pinned state to construct its target view. Intermediate
views stay pinned in their respective scopes until the drive returns. The child
executes on the target family; `AdaptedCall` executes on the original root.
Inside a child call, its receiver starts from that child's declared family.

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Compose, Direct, DirectView, Target, TargetExt};
use std::pin::Pin;

struct State(usize);
impl Compose<Direct<()>> for State {
    type Family = Direct<usize>;
    fn compose<'v, 'p>(self: Pin<&'v mut Self>, _: Pin<&'v mut DirectView<'p, ()>>)
        -> DirectView<'v, usize> where Direct<()>: 'p, Self::Family: 'v, 'p: 'v {
        DirectView(Pin::new(&mut self.get_mut().0))
    }
}
#[call]
async fn read(io: Target<Direct<usize>>) -> usize {
    io.with(|access| *access.into_pin().get_mut().0).await
}
#[call(sync)]
async fn parent(io: Target<Direct<()>>) -> usize {
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

The [sequential composition example](examples/call_composition.rs) consumes an
iterator or Stream of operations without prefetching and either yields each
completion or relays every child event. It demonstrates asynchronous waiting,
synchronous iteration, and cancellation. Run it from the repository root with
`cargo +nightly run --locked -p nitori_call --example call_composition`.

The runtime uses a stack-local dispatch slot per resume; it performs no per-poll
allocation or TLS lookup and never casts one lifetime-indexed host type to
another. Nominal coroutine output wrappers avoid a nightly inference limitation
recorded in the [compiler reproduction](../../experiments/tait-associated-output/ISSUE.md).

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

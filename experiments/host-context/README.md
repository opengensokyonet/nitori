# Asynchronous host contexts

The main workspace now implements the receiver-scope protocol and resource
drivers in `nitori_call`. This directory retains the isolated design experiments;
its resume-mode candidate predates the main implementation, which supplies mode
through the `ExecutionControl` receiver and uses the public `#[call]` macro.

This independent nightly Rust package validates lazy acquisition of a complete
family view, with one pinned view per driver poll. It does not migrate the public
`nitori_call` macros or change the existing crates' execution protocol.

## Protocol and ownership

- `HasFamily` declares capability identity without requiring reconstruction.
- `HostFamily::HostView<'view>` implements `HasFamily<Family = Self>`. It need not
  be a host capable of creating another view, nor implement `Unpin` or `Send`.
- `ViewSource<'view>` delivers an owned, complete view. `Source` creates an
  acquisition future only on demand, keeps it across pending driver polls, and
  clears it after successful delivery or cancellation.
- `Round` stores the resulting view in a pinned `Option`. `HostContext` lends
  `Pin<&'access mut F::HostView<'view>>`; repeated accesses borrow the same value.
- `HostContext::poll_ready` initializes the complete view lazily. Once ready,
  `ready_view` lends that existing value without reconstructing it. `poll_view`
  combines the two for leaf operations. All three methods support dynamic dispatch.
- `CallOn` and `AwaitOn` receive a dynamic context and task context. Ordinary
  futures ignore the host context. `Receiver` and `Child` forward it without
  rebuilding the root view. `ReceivedCall` adds a lazy projection context when
  an operation uses a different family through local receiver state.
- `Bound` owns the operation and source. Every outer Future/Stream poll creates
  one round. Pending, an outward event, completion and unwinding destroy that
  round and its view. Acquisition still pending survives only while execution
  may continue. Completion, cancellation and panic cancel outstanding acquisition.

The inner `'view` lifetime refers to external resources, not the duration of a
driver poll. A round can end much earlier; invariant view lifetimes are never
shortened or transmuted. The outer `'access` lifetime is an ordinary temporary
borrow of the context.

The source must deliver a value that owns its handle or borrows external
resources. A future cannot return a value borrowing its temporary `poll` borrow.
This interface therefore does not automatically accommodate sources whose view
borrows their own internal fields. The tested lock factories borrow an external
Tokio Mutex. Source ownership and cancellation belong to the binding; no task is
spawned by this runtime.

Acquisition has no independent error channel. Implementations must arrange
wakeup on Pending and eventual acquisition under their resource's progress
conditions. Capability operations can still return their own errors. In the
tests, lock acquisition and counted view initialization are separate steps;
initialization happens only after a guard has been obtained.

## View and event boundaries

Awaiting a `Bound` discards intermediate yields inside one round, so those yields
do not force reacquisition. Polling it as a Stream returns one event and ends the
round. Awaiting a `Child` likewise consumes its events using the existing context;
`Child::next` preserves events and unique completion.

The context caches the complete view, not just the resource guard. Capability
methods take the already acquired view directly. There is no implicit second
`Host::view` call in an operation, and there is no public `HostAction` callback
protocol or generic self-referential view storage.

`MapSource` can consume an acquired view to construct an owning outer view before
pinning. Nested maps build each layer once on successful acquisition. The outer
value owns its inner value and may borrow external state. This is a preconfigured
source chain, not a solution for arbitrary receiver state created inside a
running coroutine.

## Dynamic receiver projection

`Projection` constructs a target view from pinned receiver state and a pinned
borrow of the parent view. Its result borrows both for a common `'scope`:

```text
Pin<&'scope mut Receiver> + Pin<&'scope mut Root::HostView<'view>>
    -> Target::HostView<'scope>               where 'view: 'scope
```

This does not shorten the invariant lifetime inside the parent view. The
projection author selects a valid representation, such as reborrowing fields of
a guard-backed parent and combining them with receiver state. Identity receivers
forward the context; they do not reconstruct an owning guard from its borrow.

`ProjectedContext` borrows the parent context and receiver from outside itself.
It initially stores these two pinned references in `Option`s. Readiness checks
use a temporary reborrow of the parent; Pending retains the original loans.
After Ready, it takes both references and transfers their full `'scope` borrow
to the projection. The resulting view can then be stored in a pinned `Option`
without borrowing another field of its own context. Subsequent accesses only
reborrow that cached view. No lifetime conversion, allocation or additional
unsafe code is needed for this storage.

`ReceivedCall<R, O>` owns persistent receiver and operation state. Each drive
creates a stack-pinned projected context. Nested `ReceivedCall`s establish
nested contexts and can each build their view once, even when receiver state is
first created inside a running coroutine.

The cache has two distinct boundaries:

| Cached value | Lifetime of the cache |
| --- | --- |
| Root view | One outer `Bound` Future/Stream poll |
| Projected view | One receiver drive, until it returns to its caller |

`CallOn::poll_return` drives through discarded yields until Pending or completion.
`Bound` as a Future and `Child` as an awaitable use it. `ReceivedCall` overrides it
to keep one projection context around the entire drive; simply repeating its
`poll_call` would rebuild the projection at each discarded yield.

Explicit `Child::next` returns an event to the parent coroutine and ends that
projection scope. A subsequent `next` in the same outer poll reuses the root
view but constructs fresh projected views. The previous view must have released
its receiver-state borrow before the caller can reuse that state. This API does
not promise one projected view across all returns to the parent coroutine.
Completion, Pending, events and unwinding destroy projections from child to
parent before releasing the root view. `ReceivedCall`, `Child` and `Bound` remain
terminal if driving or destroying a view panics.

The old owned-inner `Compose` interface is not migrated here. A cached pinned
owning root view cannot be moved into it. Nested borrowed projections establish
the storage and execution path; the public receiver-chain API must choose how
to expose that path instead of assuming legacy owned-view composition applies.

## Recursive view representation

The family-indexed projection above cannot express every generic decorator. A
wrapper retaining an opaque parent needs both the short access lifetime and the
parent's original view type. A fixed `Target::HostView<'scope>` has no independent
parameter for that history. The concrete field projections in
`tests/projection.rs` do not establish this more general case.

`tests/recursive_views.rs` checks a separate candidate without replacing the
library interfaces. Its projected view has this shape:

```text
Layer<'child, Layer<'parent, RootView<'resource, 'data>>>
```

Each layer stores `Pin<&'scope mut V>` with the full, unchanged parent type `V`.
The logical family is still `Counted<V::Family>`. Capability implementations
remain on families through `ReadAt<V>`; a blanket view method forwards to them.
This changes the capability signature: a logical family no longer determines
every possible intermediate view representation by itself.

A generic projection is selected using the actual two loan types:

```text
Pin<&'scope mut Receiver>: Project<Pin<&'scope mut ParentView>>
    Output = Layer<'scope, ..., ParentView>
```

The receiver and parent are borrowed from outside the projected context, and the
result owns those loans. No field borrows another field of its context. The
context initializes its pinned output once and lends it repeatedly. It requires
only `poll_view`, with `poll_ready` defaulting to `poll_view(...).map(drop)`;
there is no separate `ready_view`. A short readiness check followed by polling
the original parent loan obtains the full borrow after sticky readiness. This
may call the lending method twice but never reconstructs a ready view.

Five runtime tests cover three generic layers, non-static family and receiver
state, invariant `!Unpin` root storage, `!Unpin` projected storage, cached views
across discarded yields, and an outer family-only dynamic operation. Ordinary
future Pending before the first access leaves every view uninitialized; Pending
after a read releases all views and the lock before the next round. A separate
retry test verifies that a pending parent leaves the original loans available.
The sequential input adapter also remembers a completed future if subsequent
view acquisition returns Pending, so later rounds do not poll that future again.
The driver must drop the root context at the round boundary; these trait methods
do not force that outer ownership policy. Projected caches still last for one
receiver drive, not all separate child calls sharing a root round.

For the canonical root representation, `RootAt<&'view Family>::View` uses an
ordinary associated type indexed by a borrow type. This permits a non-static
`dyn CallOn<Family>` bridge in the tested implementation. The positive compiler
probe additionally uses `MutexGuard<'view, Data<'data>>`, where the guard's data
really requires `'data: 'view`. Merely wrapping the earlier constrained GAT in
that trait still triggers an implied-static error on the tested nightly; the
negative probe preserves that distinction. This root bridge does not establish
family-only erasure for arbitrary intermediate representations.

## Open interface decisions

The recursive representation solves the tested storage and reborrowing cases;
it is not a complete replacement execution protocol. In particular, a single
compiler-generated coroutine has a fixed resume parameter type. It cannot
directly implement the lifetime-indexed set of
`Coroutine<ResumeEnv<Layer<'scope, V>>>` instances demanded by a generic recursive
child. The corresponding compiler probe rejects this direct encoding. Any
capability erasure used to cross that boundary must make its trait changes and
dynamic dispatch costs explicit.

`tests/recursive_coroutine.rs` tests such a boundary for one Read capability. A
stack adapter converts the concrete view loan to `dyn Read<Family = F>` on
demand, giving `ResumeEnv<F>` a fixed representation. An outer real coroutine
captures a local mutable reference, waits on an ordinary future without
requesting a view, and drives two generic receiver layers wrapping a second real
coroutine. Family implementations still provide `ReadAt<V>`; the object-safe
Read facade forwards to them. It adds capability dynamic dispatch at coroutine
boundaries. This proof uses an already acquired root view and an eager receiver;
it does not yet combine that bridge with the lazy projected contexts above. Its
adapter requires a sized concrete input view, so directly forwarding an already
erased view into another coroutine also needs separate treatment.

## Optional canonical access and configurable erasure

`tests/view_boundaries.rs` connects two boundary adapters to the library's
existing capability-independent `ResumeEnv<F>`. A stored view retains its exact
recursive type. `CanonicalAccess` explicitly constructs a separate
`F::HostView<'scope>` access value, which a stack-pinned context caches lazily for
one boundary drive. The source view and receiver storage remain outside that
context, so this adapter needs no self-referential fields or additional unsafe
code. Neither a canonical access nor a context loan survives coroutine suspension.

The static adapter borrows fields of an invariant, pinned owning guard view,
including already initialized parsed state. Recursive counter layers rebuild
only short-lived access wrappers, using one common access lifetime. The original
guard and parsed state are not reconstructed. A real coroutine waits without
constructing these wrappers, then reads twice with one construction per access
layer. This is explicit reborrowing, not covariance of the stored view. Even a
covariant pointee cannot generally have its lifetime shortened behind `&mut`;
the corresponding negative probe checks that restriction.

This option is conditional on an author-supplied canonical reborrow. It does not
let an opaque guard reproduce another owning guard, nor preserve the identity of
the outer access wrapper. It distinguishes stored view representation from
canonical call access; making the two identical again restores the original
restriction. Access wrapper construction may have costs and is only cached
within each boundary drive, not all separate children in one root round.

The erasure adapter instead uses an application-supplied schema:

```text
Schema::Interface<'scope>          the interface retained after erasure
Erase<Schema>                     a typed, safe conversion into that interface
Erased<Schema>::HostView<'scope>   one pinned interface borrow
```

The generic context, coroutine, request dispatch and access storage know no
specific capability. The tests provide a combined sequence/label interface with
a borrowed label result, and an unrelated toggle interface returning a different
result type. Both use the same coroutine driver. The combined interface operates
on an unchanged two-layer recursive view, without invoking its canonical
reborrow; operation implementations remain on logical families through typed
forwarding traits. A helper explicitly ties its schema family to borrowed local
data. The toggle test also checks lazy construction before ordinary input is ready.

This establishes configurable erasure of declared interfaces, not unrestricted
reflection over arbitrary Rust traits. A schema must choose a usable dynamic
interface; generic methods, unfixed associated types, and capability discovery
need additional design. The test uses an explicit `Erased<Schema>` boundary
family, so existing logical-family trait bounds do not carry across that boundary
unchanged. Selecting a schema as a family-associated interface is a possible
public API arrangement, not an implemented migration here. The schema adapter
itself needs no allocation or raw-pointer cast; capability calls introduce
dynamic dispatch. `Any` would not accept these non-static borrowed views.

Erasing an operation instead of a view does not automatically remove this
constraint: a `Request::execute<V>` method cannot be called through `dyn Request`.
A negative probe checks that direct encoding. A custom pointer/vtable pair still
needs a known interface or a matched, monomorphized operation/view pair; hiding a
pointer does not supply missing generic implementations.

## Lifetime-erased links with a fixed parent family

`tests/lifetime_links.rs` tests a narrower erasure boundary. At each composition,
the parent view is already known to be `F::HostView<'parent>` for one fixed F.
Only its parent lifetime needs hiding, not an arbitrary view type or its
capability set. The projected representation can therefore be:

```text
Counted<'state, F>::HostView<'scope> = CountView {
    parent: ViewLoan<'scope, F>,
    count: &'scope mut usize,
    ...
}
```

`ViewLoan` hides the original `'parent` but retains an exclusive, scope-bounded
loan. It provides access using a lifetime-generic visitor whose method receives
`Pin<&mut F::HostView<'parent>>` with `F: 'parent`. This method is generic only
over lifetimes; the exact family remains known. A typed operation and its result
are kept in a stack-local request, and the visitor invokes that operation at the
parent's original lifetime. It never substitutes `'static`, changes the parent
view's type, or reconstructs the parent view.

Repeating this representation at every layer prevents temporary ancestor view
lifetimes from accumulating in the next layer's public type. Ordinary borrowed
data and receiver-state lifetimes inside F remain unchanged. Consequently the
existing `Projection`, `ReceivedCall`, `HostContext`, `ResumeEnv<F>` and
Family-owned capability signatures can drive these views without a new schema
family or capability-specific vtable. This does not erase an F that itself
changes with every parent-view lifetime; it keeps those temporary lifetimes out
of F in the first place.

The tests combine three opaque generic counter layers, an invariant `!Unpin`
MutexGuard-owning root, non-static root and receiver families, coroutine-local
receivers and a nested real coroutine. Separate outer and inner future waits
request no view. Two operations in one drive reuse each cached view; suspension
after an access destroys all links and releases the guard before the next round.
Read and label measurement forward through the same loan implementation. A third
capability has a type-generic method accepting a non-static closure and returning
an independently borrowed result; it also needs no change to the loan framework.
Capability implementers still write their ordinary forwarding logic rather than
registering methods with an erased capability interface. The transform forwarding
also tests `loan.with(|access| F::transform(access.view, callback))`. A nominal
`Access<'access, 'parent, F>` argument carries the lifetime bounds, so this closure
adapter works with the test's non-static families and captured callback. Named
`ViewOperation<F>` implementations remain available but are not required for
each forwarded method.

This implementation adds a small unsafe boundary: a constructor pairs the
pointer with a dispatcher specialized for its exact original view type and
lifetime. Private fields, an invariant F marker, a scope borrow marker, exclusive
access, and lifetime-generic visitors enforce the intended safe interface. The
pointer is never exposed, cloned, or given Send/Sync implementations. Visits
recreate only a temporary pinned borrow of the original pointee. A visitor panic
does not corrupt the loan; a separate test checks reuse after unwinding.

There is no allocation or capability registry in this mechanism, but each link
adds indirect forwarding calls and an erased-loan field. The prototype does not
establish performance or a complete soundness proof. It
requires a known, stable Family view constructor at each boundary; it does not
provide arbitrary concrete-view downcasting or permit escaping a visit's loan.

### Effect on composition

The old public `Compose` consumes a newly constructed parent view by value.
An opaque decorator using this candidate instead stores `ViewLoan<'scope, F>`;
its capability implementations visit the parent without taking ownership of it.
Receiver state and its ordinary borrowed data stay typed and may still be pinned.
New capabilities, including type-generic methods, do not require changes to the
loan dispatcher, although decorators still implement their forwarding policy.

The existing `Composed::view` expression nests construction and returns the last
view. It cannot simply switch its input to a borrow: the returned child could
borrow a local intermediate view. A recipe chain would need to drive the child
inside nested contexts, each retaining its own intermediate view. The nested
`ReceivedCall` test demonstrates this execution pattern, not migration of the
public `.compose(...)` builder or macros.

An erased visit cannot return a reference tied to its temporary view borrow.
Therefore `ViewLoan` should not automatically replace the typed `Projection`
input for every custom composition. A projection that knows its root representation
can use the original pinned parent loan to build a field view directly. A generic
decorator can retain an erased parent loan and perform field access inside visits;
extracting a child borrow through `with` and storing it outside that visit is not
supported by the current interface. Operations returning owned results or borrows
from independently lived data are a different case.

Caching is scoped precisely: the root round caches its acquired view, while each
`ReceivedCall` drive caches its projected view. Two separate child calls through
the same receiver can construct two projected views during one root round.
Guaranteeing once per receiver per entire root round would require a longer-lived
projection scope or a different cache design; this prototype does not provide it.

## Resource drivers captured by an outer call

`ResourceDriver` describes a persistent `Execution<'driver, O>` implementing
both `Future<Output = O::Return>` and
`Stream<Item = CoroutineState<O::Yield, O::Return>>`. Its implementation chooses
its own state layout; it does not have to store a borrowing acquisition future
inside the driver struct.

`Execution` is a minimal binding for `CallOn<NoReceiver>`, whose receiver view is
`()`. A driver can construct an ordinary outer coroutine capturing itself or a
long-lived borrow of itself. The coroutine's local acquisition state may borrow
that captured driver across suspension. Compiler-generated coroutine storage
maintains these loans; no view lifetime is extended by a pointer cast, and no
hand-written self-referential resource container is introduced.

`tests/resource_driver.rs` implements both borrowed and owning Tokio Mutex
drivers, including a `!Unpin` driver and non-static data. Inside the outer call,
a source borrows the captured driver and preserves its pinned acquisition future.
Each resume drives the inner call using a fresh local round, destroys that round,
then yields Pending or an event. Completion and unwinding destroy the acquisition
state before the captured driver. Ordinary-input waits and outward events do not
cancel unfinished acquisition, even after the lock has reserved its permit.

The outer call reads `ResumeEnv::mode()` for each resume. Event mode delegates to
the inner call's `poll_call`; return mode delegates to `poll_return`, retaining a
single receiver scope across discarded inner yields. Without this distinction,
blindly forwarding every inner yield would rebuild the resource scope when an
execution is awaited as a Future. Switching between Stream and Future polling
changes the next resume's mode without replacing the persistent execution.

The tests cover repeated acquisition of an owned resource, a nested real call,
actual FIFO lock contention while another async branch progresses, event/return
mode changes, once-per-round view construction, completion cancellation, dropping
a pending execution, and panic cleanup. The driver verifies acquisition and view
destruction before its own destructor runs. The owning test suspends while an
acquisition borrows its captured driver, then reacquires after an earlier view
has been released.

This establishes a storage path for the two resource-model questions: unfinished
acquisition belongs to the whole execution, whereas acquired views belong to one
round; an owning driver can use compiler-generated coroutine storage for borrowing
acquisition state. The public macros and main workspace still need migration to
these interfaces. The experiment retains its earlier Host names for existing
fixtures; the intended public names are ReceiverFamily, ReceiverView,
ReceiverScope and ResumeContext.

## Verification coverage

`tests/execution.rs` exercises real Tokio lock contention and input-channel
readiness, partial writes, non-static data, invariant `!Unpin` views, acquisition
reuse, nested owning views, distinct Future/Stream round boundaries, completion
and cancellation with queued lock permits, and panic termination including view
and discarded-yield destructors.

`tests/coroutine.rs` drives compiler-generated coroutines through a stack-local
erased `ResumeEnv`. It checks ordinary Future waits without resource requests,
children sharing a view, suspension without replaying preceding effects, outward
events, panic termination, and transferring a suspended operation across threads
without retaining either thread's non-Send context. An end-to-end test combines
Source, Bound, a coroutine, children and an actual async Mutex view.

`tests/projection.rs` covers coroutine-local receiver state, two nested lazy
projections, siblings sharing one root view, ordinary Future and actual lock
waits, invariant non-static `!Unpin` views, discarded versus visible yields,
Pending reconstruction, dynamic call dispatch, and panic cleanup. Construction
counts and drop order verify the distinct cache boundaries.

The coroutine adapter has the same explicit unsafe contract as the current
runtime: the supplied environment is accessed only during its resume and is
consumed before every suspension. The tests use hand-written lowering. They do
not verify a migrated procedural macro. Source and round storage use safe Rust
and pin-project-lite, as do projected contexts; erased coroutine dispatch is the
unsafe boundary. These checks are not a proof of general soundness or a
performance benchmark.

Fourteen standalone compiler probes in `probes/` contain three positive controls plus
eleven expected failures. They check non-static invariant pinned view borrowing,
view escape, overlapping mutable access, moving a pinned view, shortening an
invariant lifetime, constructing another owning guard from its borrow, and two
attempts to cache a projection borrowing a parent context. The standalone
positive controls use a boxed pin for the root view; actual crate tests also
exercise stack-pinned storage. The transferred-parent-loan positive control
contrasts with the rejected short-reborrow storage shape. The failure probes
remain useful: splitting readiness alone does not make a short reborrow long
enough, or permit a context to borrow its own fields indefinitely.
The additional recursive probes cover the root-family bridge, its constrained
GAT counterexample, the fixed coroutine resume parameter boundary, covariance
under mutable borrowing, and generic request dynamic dispatch.

The package does not reconstruct owning guards, migrate the existing `Compose`
API or procedural macros, automatically convert legacy borrowed views, or define
a universal source factory API.

## Run

From this directory, with Rust nightly, rustfmt, Clippy and Python 3 installed:

```sh
cargo +nightly fetch --locked
python3 verify.py
```

The runner uses locked offline commands after dependencies have been fetched. It
checks formatting, Clippy, all test targets, doctests and compiler probes, saving
complete command output and status under `target/verification-logs` and
`target/probe-logs`. There are currently no executable doctests.

To run all runtime tests under Miri, install its nightly components and use the
separate runner:

```sh
rustup component add --toolchain nightly miri rust-src
python3 verify_miri.py
```

This runs both the default Stacked Borrows model and Tree Borrows without
disabling checks. Every invocation records complete output, tool versions,
commands and exit statuses in a new directory under `target/miri-logs`.

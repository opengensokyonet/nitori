# nitori_call

`nitori_call` runs operations written as ordinary async functions on a
resource that their caller lends for one poll at a time. An operation can await
other operations and ordinary futures, yield items, and return a final value.
It is experimental and requires the repository's pinned Rust nightly.

## Why calls?

Consider a buffer shared with the task that delivers bytes into it, such as the
receive buffer of a QUIC stream implemented in user space, and a reader of
frames made of a one-byte type, a one-byte length and the payload. An ordinary
async reader cannot keep the buffer locked while it waits for bytes, since the
delivering task needs the lock to append them. Each leaf read therefore locks,
reads what has arrived and unlocks before it waits:

```rust
# use std::io;
# use tokio::sync::Mutex;
# struct StreamBuffer;
# async fn read_array<const N: usize>(_: &Mutex<StreamBuffer>) -> io::Result<[u8; N]> {
#     Ok([0; N])
# }
# async fn read_exact(_: &Mutex<StreamBuffer>, _: &mut Vec<u8>, _: usize) -> io::Result<()> {
#     Ok(())
# }
// `read_array` and `read_exact` lock `stream`, read and unlock, and wait
// without the lock when the bytes have not arrived.
async fn read_frame(stream: &Mutex<StreamBuffer>, payload: &mut Vec<u8>) -> io::Result<u8> {
    let [kind, length] = read_array::<2>(stream).await?; // lock · read · unlock
    read_exact(stream, payload, usize::from(length)).await?; // lock · read · unlock
    Ok(kind)
}
# fn main() {}
```

When several frames have already arrived, one poll of the task runs through all
of them, and every leaf locks again. A leaf future can only choose between
holding the resource across waits and acquiring it on every poll, because only
the code that polls the composed operation knows where a poll begins and ends.
A call therefore receives the resource from its caller on each poll, and the
caller decides how long it stays acquired:

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{calls::ReadExactError, prelude::*};
use std::sync::Mutex;

#[call(sync)]
async fn read_frame<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    payload: &mut Vec<u8>,
) -> Result<u8, ReadExactError<F::Error>> {
    let [kind, length] = io.read_array::<2>().await?;
    io.read_exact(payload, usize::from(length)).await?;
    Ok(kind)
}

fn main() {
    let stream = Mutex::new(b"\x00\x03cat\x01\x00".as_slice());
    let mut payload = Vec::new();
    assert_eq!((&stream).sync_read_frame_unpin(&mut payload).unwrap(), 0);
    assert_eq!(payload, b"cat");
    assert_eq!(*stream.lock().unwrap(), b"\x01\x00"); // The next frame has not been read.
}
```

A reference to a mutex is an executor (see [Executors](#executors)): it locks
at the first read of a poll, and the reads of every frame ready in that poll
share the guard. When the bytes have not arrived, the poll ends and the lock is
released, so the delivering task can append them. Ordinary async remains
simpler when the operation owns its resource for its whole duration.

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
keep only the final value, or pin it and call `step` to handle the
items. `step` gives each item, then the final value; as a `Stream`,
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
        match chunks.as_mut().step().await {
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
and it never leaves the operation. An access is `Send` but not `Sync`: within
a poll it may move to a scoped thread, which can ask for a view, but only the
thread polling the call may use the view, and an access elsewhere panics.
Hand-written leaf operations call `poll_access`, which lends the current view
to a callback:

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

A call over several receivers, `Call<(F1, F2), Op>`, gives its operation a
tuple of slots, one for each family, and runs operations implementing
`Start<'s, (A1, A2)>`. `poll_with_views` takes a view for each slot, lent as a
`ViewLoan` or not at all: each slot asks for its own view and offers its own
back, `views_needed` and `views_offered` tell which, and a lent view wakes only
the branches waiting for that slot. A `Child` likewise runs over a tuple of
accesses.

`#[call]` makes such an operation of a function whose leading parameters are
several accesses. Inside another operation, call the function itself; an
executor over the receivers, such as a tuple of them, gets host methods:

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::prelude::*;
use std::pin::pin;

/// Copies up to `length` bytes, one at a time.
#[call(sync)]
async fn copy_bytes<F: Read, G: Write>(
    source: &mut impl ReceiverAccess<Family = F>,
    sink: &mut impl ReceiverAccess<Family = G>,
    length: usize,
) -> usize {
    let mut copied = 0;
    while copied < length {
        let Ok([byte]) = source.read_array::<1>().await else { break };
        if sink.write_all(&[byte][..]).await.is_err() {
            break;
        }
        copied += 1;
    }
    copied
}

fn main() {
    let mut hosts = pin!((b"abc".as_slice(), Vec::new()));
    assert_eq!(hosts.as_mut().sync_copy_bytes(2), 2);
    assert_eq!(hosts.1, b"ab");
}
```

With the `fn-operation` feature, which needs nightly, `fn_operation` makes an
operation of an async closure over the slots, `async |(a, b)| ..`, without
naming it. The closure accepts slots of every brand, so it can neither return
a slot nor hand one to a thread scope outside its future.

## Synchronous access

A caller that holds its resources, and lends all of them on every poll, gives
an operation more than `poll_access` promises: the view is always there. With
`ReceiverAccess`, an `.await` can mean waiting for a view as much as waiting
for an event, so the source does not show where other work can run between two
steps. `LentCall<R, Op>` states the promise in its type. It wraps a `Call`,
`poll_with_views` takes a view of every receiver, and the result is a plain
`Poll`, since no request for a view or release point can happen.

The operation takes `LentAccess`, which adds synchronous access to
`ReceiverAccess`. `.await` and `yield` are then all the points where other work
can run, and the code between them is atomic with respect to it. `io.with`
lends the view to a closure. `(&mut a, &mut b).with(|a, b| ..)`, with
`LentAccesses` imported, lends the views of 2 to 4 accesses to one closure. The
closure cannot wait and keeps no view.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{
    Direct, DirectView, LentAccess, LentAccesses, LentCall, Receiver, ReceiverFamily, call,
    capability, direct_receiver,
};
use std::{
    collections::VecDeque,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

#[capability]
pub trait Queue: ReceiverFamily {
    fn poll_pop<'v>(view: Pin<&mut Self::ReceiverView<'v>>, cx: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v;
    /// A method without a context is synchronous.
    fn remaining<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> u32
    where
        Self: 'v;
}
#[capability]
pub trait Counter: ReceiverFamily {
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, by: u32) -> u32
    where
        Self: 'v;
}

#[call]
async fn sum<Q: Queue, C: Counter>(
    queue: &mut impl LentAccess<Family = Q>,
    counter: &mut impl LentAccess<Family = C>,
    items: usize,
) -> u32 {
    let mut total = 0;
    for _ in 0..items {
        let item = queue.pop().await; // Other work can run here.
        counter.add(item); // Nothing else runs from here to the next `.await`.
        total = (&mut *queue, &mut *counter)
            .with(|mut queue, mut counter| counter.add(queue.remaining()));
    }
    total
}

#[derive(Default)]
pub struct Items(VecDeque<u32>);
direct_receiver!(impl [] for Items);
impl Queue for Direct<Items> {
    fn poll_pop<'v>(view: Pin<&mut DirectView<'v, Items>>, _: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v,
    {
        let items = view.get_pin_mut().get_mut();
        items.0.pop_front().map_or(Poll::Pending, Poll::Ready)
    }
    fn remaining<'v>(view: Pin<&mut DirectView<'v, Items>>) -> u32
    where
        Self: 'v,
    {
        view.get_pin_mut().0.len() as u32
    }
}
#[derive(Default)]
pub struct Total(u32);
direct_receiver!(impl [] for Total);
impl Counter for Direct<Total> {
    fn add<'v>(view: Pin<&mut DirectView<'v, Total>>, by: u32) -> u32
    where
        Self: 'v,
    {
        let total = view.get_pin_mut().get_mut();
        total.0 += by;
        total.0
    }
}

fn main() {
    let (mut items, mut total) = (Items(VecDeque::from([1, 2])), Total::default());
    let mut call = pin!(LentCall::<(Direct<Items>, Direct<Total>), _>::new(Sum::new(2)));
    let mut queue = pin!(Pin::new(&mut items).view());
    let mut counter = pin!(Pin::new(&mut total).view());
    let poll = call.as_mut().poll_with_views(
        (queue.as_mut(), counter.as_mut()),
        &mut Context::from_waker(Waker::noop()),
    );
    assert!(matches!(poll, Poll::Ready(ControlFlow::Break(4))));
}
```

`#[capability]` treats a method that takes a view and is not a `poll_` method as
synchronous, and a trait may have only such methods. Each one becomes a method
of `{Trait}Ext` for lent accesses, `counter.add(1)`, and of `{Trait}ViewExt` for
the `Access` a closure receives, `view.add(1)`. The second works in any closure
that holds an `Access`, including `poll_access`. A method exists on the view
alone when its signature ties anything to the view's lifetimes, as one
returning a value that borrows the view does, or when it returns `impl Trait`,
which captures them: an access keeps its view inside a closure.

`#[call]` recognizes `LentAccess` as it does `ReceiverAccess`, by the last
segment of the bound's path, so a renamed import is not seen. An operation
written for a lent access runs only in a `LentCall`: it has `{Op}Ext` for lent
accesses but no `{Op}HostExt`, since no executor promises a view on every poll,
and `sync` is an error. Every other operation works on a lent access.

A view is lent as a `Pin<&mut _>`, and the views of several receivers as a tuple
of the same shape. A `ViewLoan` can stand in for any of them.
`poll_return_with_views` delivers only the final value. A driver can lend one
pinned view to several calls in one poll, through `view.as_mut()`, and the calls
hold no resource between polls.

An operation's destructor can run outside a poll, when the call is dropped, and
there is no view then. `with` panics in that case. A destructor uses `try_with`,
which returns `None` there, on one access or on a tuple.

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
        match items.as_mut().step().await {
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

An operation over several receivers runs on executors over all of them. A
tuple of receivers lends a view of each on every poll. `drive` takes a tuple of
acquisitions and acquires each receiver only when its slot asks, on its own;
a `Constructed` receiver in the tuple lends a view on every poll instead.
`drive_composed` takes a tuple of states, one for each acquisition, so
`drive_composed((LockAcquisition::new(&a), LockAcquisition::new(&b)), (Guarded,
Guarded), operation)` runs an operation on the values behind two locks. No view
is held while the task waits: a view acquired for one slot is released at the
end of the poll even when another slot's acquisition is pending. An
acquisition that blocks, such as one of `std::sync::Mutex`, still blocks while
the poll holds the views acquired before it, so operations that lock the same
blocking locks must ask for them in the same order; asynchronous locks have no
such constraint.

An operation that needs two resources behind separate locks can also use one
family whose view holds both guards, which acquires them together in a fixed
order and lends both for the same polls, or run a nested
`drive(acquisition, operation)` for the second resource, which acquires it
only for the polls that use it; the first resource is not held while the
nested operation waits. The [driver tests](tests/driver.rs) show both, and the
[lock tests](tests/locks.rs) show operations over several receivers.

`BoundCall`, `Drive` and `Child` implement `FusedFuture` and `FusedStream`, as
`futures::select!` requires. Dropping a call cancels it without rolling back
completed IO.

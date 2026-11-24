# nitori_io

Experimental byte IO built on `nitori_call`. Requires Rust nightly (currently
verified with 1.100.0-nightly, 2026-09-19).

## Capabilities and operations

`Read`, `ReadChunk`, `Skip`, and `Write` are capabilities on `ReceiverFamily` types.
Each method accepts a pinned `Self::ReceiverView<'visit>` for any valid visit lifetime;
errors and chunks are stable associated types independent of that visit.
`Read` fills any `BufMut`; `ReadChunk: Read` additionally returns owned `Buf`
chunks from the same cursor, and `Skip: Read` advances that cursor without
delivering the bytes (see [Skipping](#skipping)). `Write` accepts any `Buf`. Buffer generics belong
to the methods. Neither hosts nor buffers require `Send`, `Sync`, `Unpin`, or
`'static`. These traits are not dyn-compatible; buffer arguments can be unsized.

`#[capability]` generates a primitive operation for each poll method: `read::Read`,
`read_chunk::ReadChunk`, `skip::Skip` and `write::Write`. Each borrows its buffer and completes
with the poll's result. The operations derived from them live in `calls`,
together with their errors. Import the generated extension traits to call them:
`ReadExt`, `ReadChunkExt`, `SkipExt`, `WriteExt` and `calls::{Name}Ext` on receiver
accesses inside operations, and `ReadHostExt`, `ReadChunkHostExt`, `SkipHostExt`,
`WriteHostExt` and
`calls::{Name}HostExt` on executors such as memory buffers and bridges.
`use nitori_io::prelude::*` imports all of them anonymously, together with the
capabilities.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{
    calls::{ReadArrayError, ReadExactError},
    prelude::*,
};
use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
enum HeaderError<E> {
    Tag { source: ReadArrayError<E> },
    Body { source: ReadExactError<E> },
}

#[call(sync)]
async fn header<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Result<[u8; 3], HeaderError<F::Error>> {
    let [tag] = io.read_array::<1>().await.context(TagSnafu)?;
    let mut bytes = [0; 2];
    let mut destination = bytes.as_mut_slice();
    io.read_exact(&mut destination, 2)
        .await
        .context(BodySnafu)?;
    Ok([tag, bytes[0], bytes[1]])
}

fn main() {
    let mut input = b"abcd".as_slice();
    assert_eq!(input.sync_header_unpin().unwrap(), *b"abc");
}
```

Crates that define operations enable `impl_trait_in_assoc_type`; using the
generated host methods needs no features. Local buffers and their cursor views
may stay borrowed across Pending.

| Operation | Final value |
| --- | --- |
| `read::Read::new(&mut destination)` | `Result<usize, F::Error>` |
| `read_chunk::ReadChunk::new(nonzero_maximum)` | `Result<Option<F::Chunk>, F::Error>` |
| `skip::Skip::new(maximum)` | `Result<Option<F::Count>, F::Error>` |
| `write::Write::new(&mut input)` | `Result<usize, F::Error>` |
| `ReadExact::new(&mut destination, length)` | `Result<(), ReadExactError<F::Error>>` |
| `ReadToEnd::new(&mut destination)` | `Result<usize, ReadToEndError<F::Error>>` |
| `ReadChunks::new(maximum, nonzero_chunk_maximum)` | yields chunks; `Result<usize, ReadChunksError<F::Error>>` |
| `ReadChunksExact::new(length, nonzero_chunk_maximum)` | yields chunks; `Result<(), ReadChunksExactError<F::Error>>` |
| `SkipExact::new(length)` | `Result<(), SkipExactError<F::Error, F::Count>>` |
| `SkipToEnd::new()` | `Result<(), F::Error>` |
| `WriteAll::new(input)` | `Result<(), WriteAllError<F::Error>>` |
| `ReadArray::<LENGTH>::new()` | `Result<[u8; LENGTH], ReadArrayError<F::Error>>` |
| `ReadLe::<T>::new()` | `Result<T, ReadLeError<F::Error>>` |
| `ReadBe::<T>::new()` | `Result<T, ReadBeError<F::Error>>` |
| `WriteArray::<LENGTH>::new(bytes)` | `Result<(), WriteArrayError<F::Error>>` |
| `WriteLe::new(value)`, `WriteBe::new(value)` | `Result<(), WriteLeError<F::Error>>`, `Result<(), WriteBeError<F::Error>>` |

The derived operations are also functions, such as `calls::read_exact(io,
destination, length)`, for use inside other operations.

Numeric operations support all integer primitives (including `usize`/`isize`)
and `f32`/`f64`, preserving floating-point bits. They read a `size_of::<T>()` array
before synchronous decoding. Pointer-sized encodings depend on the target width.

Host methods have pinned and `_unpin` entries, plus `sync_` entries for
resources that never wait. Receivers also provide `PollReadExt` and
`PollWriteExt` for one synchronous poll through their family. Access methods
work on the root access and on composed ones. A local resource can use
`nitori_call::family_receiver!` and implement the capability on itself; custom
wrapper families define their own views. Host methods also work on custom
`CallExecutor` implementations.

## Built-in hosts and bridges

| Receiver | Read | Write | Native ReadChunk | Native Skip |
| --- | --- | --- | --- | --- |
| `&[u8]` | yes | — | borrowed subslice | advance |
| `&mut [u8]` | — | bounded | — | — |
| `Bytes` | yes | — | zero-copy split | advance |
| `BytesMut` | yes | append | zero-copy split | advance |
| `Vec<u8>` | — | append | — | — |
| `VecDeque<u8>` | consume front | append | — | consume front |
| `std::io::Cursor<T>` | when `T: AsRef<[u8]> + Unpin` | when the cursor implements `std::io::Write` and `T: Unpin` | — | when `T: AsRef<[u8]> + Unpin` |

Memory operations use `Infallible`, except cursor writes which preserve
`std::io::Error`. A full slice writer returns zero. A cursor write offers one
contiguous input segment; `WriteAll` handles the remaining segments. Use a
cursor for reading a Vec without removing its prefix. Borrowed slice chunks
retain the slice's original lifetime and need not be `'static`.

`&mut T` and `Box<T>` forward supported capabilities when `T: Unpin`.
`Pin<P>` forwards when `P: DerefMut + Unpin`, without requiring its target to
be `Unpin`; this includes `Pin<&mut T>` and `Pin<Box<T>>`.

The `bridge` module adapts external IO **into** nitori capabilities:

- `bridge::Std<T>` accepts `std::io::Read` / `Write`, with the default `std`
  feature.
  It executes synchronously and may block. `WouldBlock` and `Interrupted` remain
  their original errors; no readiness registration or retry loop is invented.
- `bridge::Tokio<T>` accepts `tokio::io::AsyncRead` / `AsyncWrite` with feature
  `tokio`. It does not enable a Tokio runtime.
- `bridge::Futures<T>` accepts the traits re-exported by `futures::io` with
  feature `futures`, depending only on `futures-io`.

In the other direction, `bridge::ReadAccess<A>` wraps a receiver access inside
an operation and implements `tokio::io::AsyncRead` and `futures_io::AsyncRead`
with the corresponding feature, so codecs written for those traits can run
inside an operation. It reads through the access on each poll, buffers nothing,
and converts the family's read errors into `std::io::Error`. Writing is not yet
adapted, since flush and close are not part of `Write`.

The default feature is `std`, which adds the `bridge` module and the
`std::io::Cursor` hosts; `tokio` and `futures` enable it. Without it the crate
builds without the standard library, with `alloc`. Both optional features may
be enabled together. Async bridges
support pinned, borrowed, and non-Send hosts. Each wrapper provides `new`,
`From<T>`, `get_ref`, `get_mut`, and `into_inner`; async wrappers additionally
provide `get_pin_mut`, preserving the pinning of a `!Unpin` async host.

Bridge reads use an initialized stack buffer of at most 8 KiB, then copy only
successful bytes to the caller's arbitrary `BufMut`. This preserves the buffer
on Pending and errors without unsafe code. They retain no unread bytes or
per-poll buffer pointers. Writes pass one contiguous input segment directly to
the host. Empty requests still reach the host. Flush, close, and seek remain
explicit operations on the underlying object; dropping a bridge adds no flush.

`adapters::Chunked<T>` adds copying `ReadChunk` to a host whose family implements `Read`, including a bridge,
with an explicit nonzero chunk capacity. It caps each read by both that capacity
and the requested maximum, retains empty allocated storage across Pending, and
returns owned `Bytes` on success. A larger subsequent request replaces scratch
storage if necessary instead of growing it geometrically beyond the limit. Ordinary Read and Write calls pass through to
the same host. It does not prefetch, and cancellation cannot lose an unread tail.
Allocation uses `BytesMut`'s infallible allocation policy; the allocator may round
capacity up. Native chunk hosts do not need this adapter.

```rust
use nitori_io::{adapters::Chunked, bridge::Std, PollReadExt};
use std::{io::Cursor, num::NonZeroUsize, pin::Pin, task::{Context, Poll, Waker}};

let mut io = Chunked::new(
    Std::new(Cursor::new(b"hello")),
    NonZeroUsize::new(1024).unwrap(),
);
let mut cx = Context::from_waker(Waker::noop());
let Poll::Ready(Ok(Some(chunk))) = Pin::new(&mut io)
    .poll_read_chunk(&mut cx, NonZeroUsize::new(3).unwrap())
else { panic!("synchronous memory IO completes immediately") };
assert_eq!(chunk, &b"hel"[..]);
assert_eq!(io.into_inner().into_inner().position(), 3);
```

## Exact remaining length

A family implementing `ExactLen` can report the exact byte length from its
current cursor to normal EOF without reading. Its associated `Length` is
chosen by the source. Memory readers report `usize`. A source that never
reaches normal EOF may report the zero-sized `Infinite`. A subsequent IO
failure can still end a read early. Ordinary readers need only implement
the capabilities they provide.

The length query is a synchronous view method. It is available through the
prelude on a lent access or on the view obtained by `poll_access`.

```rust
#![feature(impl_trait_in_assoc_type)]
use core::{future::poll_fn, task::Poll};
use nitori_call::{ReceiverAccess, call};
use nitori_io::{adapters::ReadLimit, prelude::*};

#[call(sync)]
async fn bounded_len<F: ExactLen<Length = usize>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> u8 {
    let mut limited = io.compose(ReadLimit(4u8));
    poll_fn(|cx| limited.poll_access(cx, |mut view, _| Poll::Ready(view.remaining_len()))).await
}
let mut input = &b"abc"[..];
assert_eq!(input.sync_bounded_len_unpin(), 3u8);
assert_eq!(input, b"abc");
```

Slices, `Bytes`, `BytesMut`, `VecDeque<u8>`, and the standard-library
`Cursor` over byte arrays, slices, vectors, boxed slices, `Bytes`, or
`BytesMut` support the query. Seeking a cursor past its end gives zero.
`Chunked` forwards its inner source's exact length and type.

## Byte budgets

`adapters::ReadLimit(n)` and `adapters::WriteLimit(n)` are byte budgets for
`io.compose(..)` inside an operation. The composed access reads or accepts at
most `n` more bytes; past the budget a read reports EOF and a write accepts
nothing, which `write_all` reports as zero progress; the composed access
answers there without polling the parent. The budget survives Pending and cancellation, and `into_state` returns
what remains. A budget kept with a receiver,
`nitori_call::ComposedReceiver::new(receiver, ReadLimit(n))`, bounds every
call on that receiver together and outlives each of them. A read budget forwards the parent's read capabilities and can
also report an exact length when the parent supplies one. A write budget
forwards `Write`.

`ReadLimit<L = usize>` stores a logical quota of type `L`. All unsigned
integer widths and `Infinite` are supported. A single transfer is still
bounded by `usize`. With a finite quota on an unknown source, early normal
EOF can leave unused quota. With an infinite source, the exact length of
the limited view equals the quota. A bare integer may need its type written
explicitly, for example `ReadLimit(6usize)`.

`Min<Rhs>` takes both operands by value and has an associated `Output`.
For the built-in lengths, a finite right operand selects the result type and
`Infinite` on the right preserves the left type. A source's length alone
needs no arithmetic bounds. The adapter clones its live quota for querying
and constraining a request. A custom quota `L` implements
`Min<usize, Output = usize>` to bound one read. When the host succeeds,
the adapter deducts the actual count with standard `SubAssign`. It expresses
that count in the subtraction operand type using `Min::min(count, quota)`:
the count already fits the quota. Querying a limited source additionally
requires `F::Length: Min<L>`. Querying the original source needs none of
these quota bounds.

For example, a write budget can check a bounded payload:

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{adapters::WriteLimit, calls::WriteAllError, prelude::*};

/// Write `payload` only if it fits in `room` bytes; report what is left.
#[call(sync)]
async fn write_bounded<F: Write, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    payload: &[u8],
    room: usize,
) -> (usize, Result<(), WriteAllError<F::Error>>) {
    let mut bounded = io.compose(WriteLimit(room));
    let result = bounded.write_all(payload).await;
    (bounded.into_state().0, result)
}

fn main() {
    let mut output = Vec::new();
    let (left, result) = output.sync_write_bounded_unpin(b"abc", 2);
    assert_eq!((left, output.as_slice()), (0, &b"ab"[..]));
    assert_eq!(result.unwrap_err().completed(), 2);
}
```

## Skipping

`Skip` advances past bytes without delivering them. Its associated `Count`
is chosen by the source: memory hosts skip natively, by advancing their
cursor, and report `usize`. Each `poll_skip` takes a maximum, a finite
count of any unsigned width or `Infinite`, and skips `maximum.min(available)`
bytes by the `Min` rules above, so the result has the source's `Count` type.
`Some(n)` is the count this poll skipped, at most the maximum, and `None` is
EOF. As with reads, the count is exactly what was consumed and `Pending` and
`Err` consume nothing, so a budget or a payload's remainder kept above the
source stays in step with it, cancellation included. A zero maximum is an
empty request: `Some(0)` reports no progress and says nothing about EOF, and
the request still reaches the host.

A loan of a view, `Chunked` and a read budget forward `Skip` with the
parent's `Count`; a budget bounds each skip by itself too and reports EOF at
its boundary without polling the parent. For it, the quota must also satisfy
`L: Min<F::Count, Output = F::Count>` and `F::Count: Min<L> + Clone`, with
`L: SubAssign` of that minimum, which the built-in widths and `Infinite` do.

A reader that cannot skip natively gets `Skip` from a composition state,
on an access with `io.compose(..)` or on a receiver with
`ComposedReceiver::new(..)`. `adapters::ReadSkip` reads at most 8 KiB into a
stack buffer on each poll and drops it; `adapters::ReadChunkSkip` takes one
chunk of at most the maximum and drops it, without copying. Both skip with
`Count = usize`, hold no state, and forward `Read`, `ReadChunk` and
`ExactLen` of the parent. There is no blanket implementation for readers, so
a source that can skip natively implements `Skip` itself.

`calls::skip_exact(io, length)` skips exactly `length`, asking for at most
the remainder on each poll; on EOF or a host failure its `SkipExactError`
reports the count still left. It needs `Count: Clone + Min<Output = Count> +
Min<usize, Output = usize> + SubAssign`. `calls::skip_to_end(io)` skips to
EOF with an `Infinite` maximum and reports no total, which need not fit the
`Count`; its only failure is the host's own error.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ComposedReceiver, ReceiverAccess, call};
use nitori_io::{
    adapters::{ReadLimit, ReadSkip},
    bridge::Std,
    calls::SkipExactError,
    prelude::*,
};
use std::io::Cursor;

/// Skip the first `header` bytes of a frame of `length`, then the rest.
#[call(sync)]
async fn skip_frame<F: Skip<Count = usize>>(
    io: &mut impl ReceiverAccess<Family = F>,
    length: usize,
    header: usize,
) -> (Result<(), SkipExactError<F::Error, usize>>, Result<(), F::Error>) {
    let mut frame = io.compose(ReadLimit(length));
    let header = frame.skip_exact(header).await;
    (header, frame.skip_to_end().await)
}

let mut input = &b"framenext"[..];
let (header, rest) = input.sync_skip_frame_unpin(5, 2);
assert!(header.is_ok() && rest.is_ok());
assert_eq!(input, b"next");

// A reader without native skipping skips by reading.
let mut reader = ComposedReceiver::new(Std::new(Cursor::new(b"abcdef")), ReadSkip);
assert_eq!(reader.sync_skip_unpin(4u8).unwrap(), Some(4usize));
```

## Splitting a buffer

`SplitTo` cuts the first bytes of a buffer off as a piece of their own and
leaves the rest in the buffer, without copying. It is implemented for `Bytes`,
whose pieces are read-only, for `BytesMut`, whose pieces are writable
independently, and for `&[u8]`, whose pieces are subslices. The trait does not
require `AsRef<[u8]>` or `AsMut<[u8]>`; code that reads the bytes or rewrites
them in place adds the bound it needs. Cutting more bytes than the buffer holds
returns a `SplitToError` that carries both numbers and leaves the buffer
unchanged. Unlike `ReadChunk`, a piece is one contiguous buffer, which is what
in-place header unprotection and AEAD need.

```rust
use nitori_io::SplitTo;

let mut datagram = &b"headbody"[..];
let head = datagram.split_to(4).unwrap();
assert_eq!((head, datagram), (&b"head"[..], &b"body"[..]));
```

## Progress, errors, and cancellation

Basic polls report a completed prefix before a later error. `Pending` and `Err`
consume no new input and do not advance buffer cursors. Hosts register wakeups
before Pending and may not retain pointers into a poll's temporary buffer borrow.
Resource registration and its cleanup remain the host's responsibility.

`ReadExact` checks full destination capacity on its first poll, before touching
the source, and limits subsequent reads to the remaining length. `ReadToEnd`
reports only newly appended bytes. If its destination fills before EOF is
confirmed, it reports a `DestinationFullError`, even for an exactly fitting source; it
does not consume a probe byte.

Each derived operation has its own error type in `calls`, defined alongside
the operation and containing only failures that operation can produce.
`IncompleteError`, `InsufficientCapacityError`, `DestinationFullError`, and
`WriteZeroError` are reusable error primitives in `error`. `IncompleteError` and
`InsufficientCapacityError` keep their fields private so that the completed count
is always below the expected one and the available room below the required one;
`IncompleteError::new` checks the former. Match operation error variants directly to recover their structured
fields; `completed()` reports prior progress and
`host_error()` borrows the original host failure. `ReadChunksError` always
contains a host error, so its accessor returns `&E`; enum accessors return
`Option<&E>`.

All operation errors derive SNAFU. Construction and progress access impose no
diagnostic bounds on host errors, including borrowed payloads without `Debug`
or `Display`. Standard `Error` and the original source chain are available
when the host error implements `Error + 'static`. Primitive variants delegate
diagnostics transparently; host variants retain the original source directly.
Array/numeric failures report progress but do not return the partial array.

`write::Write` borrows its input cursor and advances it by the accepted count.
`WriteAll` takes any `Buf` by value; pass `&mut buffer` to keep the cursor,
which after a failure or cancellation holds the unaccepted tail. Its errors
report the total accepted bytes. A nonempty successful zero write is a
`WriteZeroError` failure. Accepted bytes do not imply flushing or remote delivery.

Derived operations retain progress across Pending. Dropping a call does not
roll back earlier successful reads or writes. External borrowed cursors retain
their progress; owned inputs, partial arrays, and undelivered owned storage are
dropped with their owner. Cancellation has no ordinary completion result.

Empty basic Read/Write requests still reach the host, allowing terminal errors
to take precedence. Zero-length Exact/Array/Chunks and an empty WriteAll complete
locally without checking the host. ReadToEnd with zero destination capacity
fails locally. Empty basic reads do not establish EOF.

## Chunks and conversion helpers

Chunk sequences yield one nonempty bounded chunk per poll and never collect
the whole sequence. Read an execution as a Stream to receive
`ControlFlow::Continue(chunk)` for each chunk, then one `Break` with the final
value; awaiting it drops the chunks. Inside an operation, pin the child and
await `child.as_mut().next()` to relay its chunks and observe its final value.

`helpers::poll_read_from_chunk` copies a single bounded chunk into the destination.
The host must perform its terminal-error checks before delegating, especially
for an empty destination. `helpers::poll_chunk_from_read` takes explicit
`&mut Option<Storage>`, creation, and finalization arguments. It retains empty
storage across Pending, caps each read to the current request, and moves storage
to finalization on success. The finalizer exposes exactly the new bytes. Use a
bounded BufMut view when limiting allocation: growable Vec/BytesMut capacity is
not a hard `remaining_mut()` limit. Fallible allocation and allocation-error
mapping belong to the caller, which can prepare storage in advance. Neither
helper adds blanket impls or recursive defaults.

Flush/Close contracts, dynamic host adaptation, and higher-level codecs
remain outside this first implementation.

## Verification

From the repository root, run the checks listed in the root README. The focused
suite is `cargo test --locked -p nitori_io`; it includes execution on hosts,
operations written with `#[call]`, Pending/wakeup behavior, typed errors, buffer
ownership, cancellation, conversion helpers, and numeric decoding.

For the optional bridges, also run `cargo test --locked -p nitori_io --features tokio`, `cargo test --locked -p nitori_io --features futures`,
and `cargo test --locked -p nitori_io --all-features`. The bridge tests
exercise pinned non-Send hosts, wakeups, error identity, partial progress,
segmented buffers, cancellation followed by a different chunk maximum, and
codecs for the async reader traits running inside an operation.

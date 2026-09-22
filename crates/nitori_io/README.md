# nitori_io

Experimental byte IO built on `nitori_call`. Requires Rust nightly (currently
verified with 1.100.0-nightly, 2026-09-19). The crate uses `read_le` for synchronous
numeric decoding and does not reference `std::io::FromEndianBytes`.

## Capabilities and operations

`Read`, `ReadChunk`, and `Write` are capabilities on `ReceiverFamily` types.
Each method accepts a pinned `Self::ReceiverView<'visit>` for any valid visit lifetime;
errors and chunks are stable associated types independent of that visit.
`Read` fills any `BufMut`; `ReadChunk: Read` additionally returns owned `Buf`
chunks from the same cursor. `Write` accepts any `Buf`. Buffer generics belong
to the methods. Neither hosts nor buffers require `Send`, `Sync`, `Unpin`, or
`'static`. These traits are not dyn-compatible; buffer arguments can be unsized.

Import receiver extension traits for child calls, and operation types from
`nitori_io::calls` when constructing operations explicitly:

```rust
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{call, Target};
use nitori_io::{Read as ReadHost, TargetReadExt};
use nitori_io::calls::ReadExactError;

#[call]
async fn header<H: ReadHost>(io: Target<H>) -> Result<[u8; 3], ReadExactError<H::Error>> {
    let [tag] = io.read_array::<1>().await?;
    let mut bytes = [0; 2];
    let mut destination = bytes.as_mut_slice();
    io.read_exact(&mut destination, 2).await?;
    Ok([tag, bytes[0], bytes[1]])
}
# fn main() {}
```

Consumers of `#[call]` need the feature gates above, but no direct
pin-projection dependency. Bind an operation explicitly with
`BoundCall::new(pinned_host, ReadArray::<3>::new())` to execute outside a call body. Local buffers and their named cursor views may remain borrowed across
Pending. Bind temporary buffer views to locals before passing their borrows to
child calls.

| Operation constructor | Completion |
| --- | --- |
| `Read::new(&mut destination)` | `Result<usize, H::Error>` |
| `ReadChunk::new(nonzero_maximum)` | `Result<Option<H::Chunk>, H::Error>` |
| `Write::new(input)` | `WriteReturn<Input, H::Error>` |
| `ReadExact::new(&mut destination, length)` | `Result<(), ReadExactError<H::Error>>` |
| `ReadToEnd::new(&mut destination)` | `Result<usize, ReadToEndError<H::Error>>` |
| `ReadChunks::new(maximum, nonzero_chunk_maximum)` | yields chunks; `Result<usize, ReadChunksError<H::Error>>` |
| `ReadChunksExact::new(length, nonzero_chunk_maximum)` | yields chunks; `Result<(), ReadChunksExactError<H::Error>>` |
| `WriteAll::new(input)` | `WriteReturn<Input, WriteAllError<H::Error>>` |
| `ReadArray::<LENGTH>::new()` | `Result<[u8; LENGTH], ReadArrayError<H::Error>>` |
| `ReadLe::<T>::new()` | `Result<T, ReadLeError<H::Error>>` |
| `ReadBe::<T>::new()` | `Result<T, ReadBeError<H::Error>>` |

Numeric operations support all integer primitives (including `usize`/`isize`)
and `f32`/`f64`, preserving floating-point bits. They read a `size_of::<T>()` array
before synchronous decoding. Pointer-sized encodings depend on the target width.

Actual hosts provide `ReadExt`, `ChunkExt`, and `WriteExt` operation methods
returning bound calls, with pinned and `_unpin` entries. They also provide `PollReadExt` / `PollWriteExt` for synchronous poll
access through their canonical family. `TargetReadExt`, `TargetChunkExt`,
and `TargetWriteExt` work on both root and composed receivers. A local
resource can use `nitori_call::family_receiver!` and implement the family capability
on itself; custom wrapper families define their own views and capability identity.
IO operations request the current scope lazily and invoke capabilities on its
cached view, without reconstructing it. Resource extension methods also work
on custom `ResourceDriver` implementations.

## Built-in hosts and bridges

| Receiver | Read | Write | Native ReadChunk |
| --- | --- | --- | --- |
| `&[u8]` | yes | — | borrowed subslice |
| `&mut [u8]` | — | bounded | — |
| `Bytes` | yes | — | zero-copy split |
| `BytesMut` | yes | append | zero-copy split |
| `Vec<u8>` | — | append | — |
| `VecDeque<u8>` | consume front | append | — |
| `std::io::Cursor<T>` | when `T: AsRef<[u8]> + Unpin` | when the cursor implements `std::io::Write` and `T: Unpin` | — |

Memory operations use `Infallible`, except cursor writes which preserve
`std::io::Error`. A full slice writer returns zero. A cursor write offers one
contiguous input segment; `WriteAll` handles the remaining segments. Use a
cursor for reading a Vec without removing its prefix. Borrowed slice chunks
retain the slice's original lifetime and need not be `'static`.

`&mut T` and `Box<T>` forward supported capabilities when `T: Unpin`.
`Pin<P>` forwards when `P: DerefMut + Unpin`, without requiring its target to
be `Unpin`; this includes `Pin<&mut T>` and `Pin<Box<T>>`.

The `bridge` module adapts external IO **into** nitori capabilities:

- `bridge::Std<T>` accepts `std::io::Read` / `Write`, with no optional feature.
  It executes synchronously and may block. `WouldBlock` and `Interrupted` remain
  their original errors; no readiness registration or retry loop is invented.
- `bridge::Tokio<T>` accepts `tokio::io::AsyncRead` / `AsyncWrite` with feature
  `tokio`. It does not enable a Tokio runtime.
- `bridge::Futures<T>` accepts the traits re-exported by `futures::io` with
  feature `futures`, depending only on `futures-io`.

Default features are empty. Both optional features may be enabled together.
The crate still requires std when default features are disabled. Async bridges
support pinned, borrowed, and non-Send hosts. Each wrapper provides `new`,
`From<T>`, `get_ref`, `get_mut`, and `into_inner`; async wrappers additionally
provide `get_pin_mut`, preserving the pinning of a `!Unpin` async host.

Bridge reads use an initialized stack buffer of at most 8 KiB, then copy only
successful bytes to the caller's arbitrary `BufMut`. This preserves the buffer
on Pending and errors without unsafe code. They retain no unread bytes or
per-poll buffer pointers. Writes pass one contiguous input segment directly to
the host. Empty requests still reach the host. Flush, close, and seek remain
explicit operations on the underlying object; dropping a bridge adds no flush.
Reverse conversion is not provided.

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

## Progress, errors, and cancellation

Basic polls report a completed prefix before a later error. `Pending` and `Err`
consume no new input and do not advance buffer cursors. Hosts register wakeups
before Pending and may not retain pointers into a poll's temporary buffer borrow.
Resource registration and its cleanup remain the host's responsibility.

`ReadExact` checks full destination capacity on its first poll, before touching
the source, and limits subsequent reads to the remaining length. `ReadToEnd`
reports only newly appended bytes. If its destination fills before EOF is
confirmed, it reports capacity failure, even for an exactly fitting source; it
does not consume a probe byte.

Each composite operation has its own error type in `calls`, defined alongside
the operation and containing only failures that operation can produce.
`Incomplete`, `InsufficientCapacity`, and `WriteZero` are reusable error primitives
in `error`. Match operation error variants directly to recover their structured
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

`WriteReturn` contains `input` and `result`. Owned input or a borrowed cursor is
returned on success and ordinary failure. `WriteAll` errors report total accepted
bytes and preserve the remaining tail. A nonempty successful zero write is a
`WriteZero` failure. Accepted bytes do not imply flushing or remote delivery.

Derived operations retain progress across Pending. Dropping a call does not
roll back earlier successful reads or writes. External borrowed cursors retain
their progress; owned inputs, partial arrays, and undelivered owned storage are
dropped with their owner. Cancellation has no ordinary completion result.

Empty basic Read/Write requests still reach the host, allowing terminal errors
to take precedence. Zero-length Exact/Array/Chunks and an empty WriteAll complete
locally without checking the host. ReadToEnd with zero destination capacity
fails locally. Empty basic reads do not establish EOF.

## Chunks and conversion helpers

Chunk sequences yield one nonempty bounded chunk per resumption and never collect
the whole sequence. Use `BoundCall` as a Stream or its pinned `next()` method to
receive `CoroutineState::Yielded` followed by one `Complete` event. Directly
awaiting a BoundCall discards yields. Inside a call, pin the child and await `child.as_mut().next()` to relay its
yields and observe its completion.

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

Flush/Close contracts, Skip, dynamic host adaptation, and higher-level codecs
remain outside this first implementation.

## Verification

From the repository root, run the checks listed in the root README. The focused
suite is `cargo +nightly test --locked -p nitori_io`; it includes real BoundCall
execution, family macro calls, Pending/wakeup behavior, typed errors, buffer
ownership, cancellation, conversion helpers, and numeric decoding.

For the optional bridges, also run `cargo +nightly test --locked -p nitori_io --features tokio`, `cargo +nightly test --locked -p nitori_io --features futures`,
and `cargo +nightly test --locked -p nitori_io --all-features`. The bridge tests
exercise pinned non-Send hosts, wakeups, error identity, partial progress,
segmented buffers, and cancellation followed by a different chunk maximum.

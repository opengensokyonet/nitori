# nitori_io

Experimental byte IO built on `nitori_call`. Requires Rust nightly (currently
verified with 1.100.0-nightly, 2026-09-19). The crate uses `read_le` for synchronous
numeric decoding and does not reference `std::io::FromEndianBytes`.

## Capabilities and operations

`Read` fills any `BufMut`; `ReadChunk: Read` additionally returns owned `Buf`
chunks from the same cursor. `Write` accepts any `Buf`. Buffer generics belong
to the methods. Neither hosts nor buffers require `Send`, `Sync`, `Unpin`, or
`'static`. These traits are not dyn-compatible; buffer arguments can be unsized.

Import operation types from `nitori_io::calls` when using virtual calls:

```rust
#![feature(coroutines, coroutine_trait, stmt_expr_attributes, type_alias_impl_trait)]
use nitori_call::call;
use nitori_io::{Read as ReadHost, calls::{ReadArray, ReadExact}};
use nitori_io::error::ReadError;
use std::pin::Pin;

#[call]
async fn header<H: ReadHost + ?Sized>(io: Pin<&mut H>) -> Result<[u8; 3], ReadError<H::Error>> {
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
`BoundCall::new(pinned_host, ReadArray::<3>::new())` to execute outside a virtual
call. Local buffers and their named cursor views may remain borrowed across
Pending. Bind temporary buffer views to locals before passing their borrows to
virtual calls.

| Operation constructor | Completion |
| --- | --- |
| `Read::new(&mut destination)` | `Result<usize, H::Error>` |
| `ReadChunk::new(nonzero_maximum)` | `Result<Option<H::Chunk>, H::Error>` |
| `Write::new(input)` | `WriteReturn<Input, H::Error>` |
| `ReadExact::new(&mut destination, length)` | `Result<(), ReadError<H::Error>>` |
| `ReadToEnd::new(&mut destination)` | `Result<usize, ReadError<H::Error>>` |
| `ReadChunks::new(maximum, nonzero_chunk_maximum)` | yields chunks; completes with actual byte count |
| `ReadChunksExact::new(length, nonzero_chunk_maximum)` | yields chunks; completes with `()` |
| `WriteAll::new(input)` | `WriteReturn<Input, WriteError<H::Error>>` |
| `ReadArray::<LENGTH>::new()` | `Result<[u8; LENGTH], ReadError<H::Error>>` |
| `ReadLe::<T>::new()`, `ReadBe::<T>::new()` | `Result<T, ReadError<H::Error>>` |

Numeric operations support all integer primitives (including `usize`/`isize`)
and `f32`/`f64`, preserving floating-point bits. They read a `size_of::<T>()` array
before synchronous decoding. Pointer-sized encodings depend on the target width.

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

Derived errors expose `failure()`, `completed()`, `host_error()`, and
`into_parts()`. SNAFU-derived `ReadFailure`/`WriteFailure` describe capacity,
premature EOF, host failure, and zero write progress. Wrappers retain arbitrary
host error types, including borrowed errors; they implement standard `Error`
with the original source chain when the host error implements `Error + 'static`.
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
awaiting a BoundCall discards yields. Virtual forwarding of yielding child calls
is not yet supported by `nitori_call`.

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
execution, virtual macro calls, Pending/wakeup behavior, typed errors, buffer
ownership, cancellation, conversion helpers, and numeric decoding.

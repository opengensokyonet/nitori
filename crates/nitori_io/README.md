# nitori_io

Experimental byte IO built on `nitori_call`. Requires Rust nightly (currently
verified with 1.100.0-nightly, 2026-09-19).

## Capabilities and operations

`Read`, `ReadChunk`, `Skip`, `Write`, and `WriteLent` are capabilities on `ReceiverFamily` types.
Each method accepts a pinned `Self::ReceiverView<'visit>` for any valid visit lifetime;
errors and chunks are stable associated types independent of that visit.
`Read` fills any `BufMut`; `ReadChunk: Read` additionally returns owned `Buf`
chunks from the same cursor, and `Skip: Read` advances that cursor without
delivering the bytes (see [Skipping](#skipping)). `Write<O>` accepts any `Buf`
and writes it with the write options `O`, `()` for a sink that takes none;
`WriteLent<O>: Write<O>` lends the sink's own storage to be filled in place (see
[Lending storage](#lending-storage)). Reads report their bytes in segments with
metadata and writes with a report (see [Metadata and reports](#metadata-and-reports)),
and each segment is written with its options (see [Write options](#write-options)). Buffer generics belong
to the methods. Neither hosts nor buffers require `Send`, `Sync`, `Unpin`, or
`'static`. These traits are not dyn-compatible; buffer arguments can be unsized.

`#[capability]` generates a primitive operation for each poll method: `read::Read`,
`read_chunk::ReadChunk`, `skip::Skip` and `write::Write`. Each borrows its buffer and completes
with the poll's result. `WriteLent` has none: its fill runs within one poll and
is dropped on `Pending`, so an operation that lends calls `poll_write_lent`
inside the sink access's `poll_access`, as `calls::copy_into_lent` does. The operations derived from them live in `calls`,
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
async fn header<F: Read<Metadata = ()>, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Result<[u8; 3], HeaderError<F::Error>> {
    let [tag] = io.read_array::<1>().await.context(TagSnafu)?.value;
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

In the table `M` is `F::Metadata`, and for writes with options of type `O`,
`R` is `<F as Write<O>>::Report` and `F::Error` is `<F as Write<O>>::Error`.

| Operation | Final value |
| --- | --- |
| `read::Read::new(&mut destination)` | `Result<Option<ReadSegment<M>>, F::Error>` |
| `read_chunk::ReadChunk::new(nonzero_maximum)` | `Result<Option<ChunkSegment<F::Chunk, M>>, F::Error>` |
| `skip::Skip::new(maximum)` | `Result<Option<F::Count>, F::Error>` |
| `write::Write::new(&mut input, &options)` | `Result<Option<WriteSegment<R>>, F::Error>` |
| `ReadExact::new(&mut destination, length)` | `Result<M, ReadExactError<F::Error>>` |
| `ReadToEnd::new(&mut destination)` | `Result<ReadTotal<M>, ReadToEndError<F::Error>>` |
| `ReadChunks::new(maximum, nonzero_chunk_maximum)` | yields `ChunkSegment`s; `Result<usize, ReadChunksError<F::Error>>` |
| `ReadChunksExact::new(length, nonzero_chunk_maximum)` | yields `ChunkSegment`s; `Result<(), ReadChunksExactError<F::Error>>` |
| `SkipExact::new(length)` | `Result<(), SkipExactError<F::Error, F::Count>>` |
| `SkipToEnd::new()` | `Result<(), F::Error>` |
| `WriteAll::new(input, options)` | `Result<R, WriteAllError<F::Error>>` |
| `ReadArray::<LENGTH>::new()` | `Result<Value<[u8; LENGTH], M>, ReadArrayError<F::Error>>` |
| `ReadLe::<T>::new()` | `Result<Value<T, M>, ReadLeError<F::Error>>` |
| `ReadBe::<T>::new()` | `Result<Value<T, M>, ReadBeError<F::Error>>` |
| `WriteArray::<LENGTH>::new(bytes, options)` | `Result<R, WriteArrayError<F::Error>>` |
| `WriteLe::new(value, options)`, `WriteBe::new(value, options)` | `Result<R, WriteLeError<F::Error>>`, `Result<R, WriteBeError<F::Error>>` |

The operations that combine segments need `M: Combine` or `R: Combine`. The
writes take their options by value, keep them across Pending and lend each
segment's poll `&options`; call `io.write_all(input, ())` on a sink without
options. `Value { value, metadata }`, at the crate root, is a value with the
metadata of its bytes. The copies, which take a source and a sink, are listed
under [Copying](#copying).

The derived operations are also functions, such as `calls::read_exact(io,
destination, length)`, for use inside other operations.

Numeric operations support all integer primitives (including `usize`/`isize`)
and `f32`/`f64`, preserving floating-point bits. They read a `size_of::<T>()` array
before synchronous decoding. Pointer-sized encodings depend on the target width.

Host methods have pinned and `_unpin` entries, plus `sync_` entries for
resources that never wait. Receivers also provide `PollReadExt` and
`PollWriteExt` for one synchronous poll through their family; the latter
includes `poll_write_lent`. Access methods
work on the root access and on composed ones. A local resource can use
`nitori_call::family_receiver!` and implement the capability on itself; custom
wrapper families define their own views. Host methods also work on custom
`CallExecutor` implementations.

## Built-in hosts and bridges

| Receiver | Read | Write | Native ReadChunk | Native Skip | WriteLent |
| --- | --- | --- | --- | --- | --- |
| `&[u8]` | yes | — | borrowed subslice | advance | — |
| `&mut [u8]` | — | bounded | — | — | the unwritten rest |
| `Bytes` | yes | — | zero-copy split | advance | — |
| `BytesMut` | yes | append | zero-copy split | advance | spare capacity, split off |
| `Vec<u8>` | — | append | — | — | spare capacity, `VecSpareCapacity` |
| `VecDeque<u8>` | consume front | append | — | consume front | — |
| `std::io::Cursor<T>` | when `T: AsRef<[u8]> + Unpin` | when the cursor implements `std::io::Write` and `T: Unpin` | — | when `T: AsRef<[u8]> + Unpin` | — |

Memory hosts take no write options and have no metadata and no report:
they write with `Write<()>` and `WriteLent<()>`, and the rest are `()`. Memory
operations use `!`, except cursor writes which preserve
`std::io::Error`. A full slice writer accepts nothing, `None`. A cursor write offers one
contiguous input segment; `WriteAll` handles the remaining segments. Use a
cursor for reading a Vec without removing its prefix. Borrowed slice chunks
retain the slice's original lifetime and need not be `'static`. A lending
`BytesMut` splits its spare capacity off and lends it as an empty buffer; a
lending `Vec<u8>` lends its spare capacity as a `VecSpareCapacity`, which
counts what is put into it and becomes part of the vector only when kept.
Either way the fill cannot reach the bytes written before, and nothing is
copied or zeroed.

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

In the other direction, `bridge::ReadAccess<A, M>` wraps a receiver access inside
an operation and implements `tokio::io::AsyncRead` and `futures_io::AsyncRead`
with the corresponding feature, so codecs written for those traits can run
inside an operation. It reads through the access on each poll, buffers nothing,
and converts the family's read errors into `std::io::Error`. The async reader
traits carry no metadata, so it keeps the family's: `metadata()` is the
metadata of every segment read, merged with `Combine`, and `take_metadata()`
hands it over, for example once per decoded message. Writing is not yet
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
the host; the adapted writers take no options, so the bridges write with
`Write<()>`. Empty requests still reach the host. Flush, close, and seek remain
explicit operations on the underlying object; dropping a bridge adds no flush.

`adapters::Chunked<T>` adds copying `ReadChunk` to a host whose family implements `Read`, including a bridge,
with an explicit nonzero chunk capacity. It caps each read by both that capacity
and the requested maximum, retains empty allocated storage across Pending, and
returns owned `Bytes` on success, with the metadata of the read that filled it. A larger subsequent request replaces scratch
storage if necessary instead of growing it geometrically beyond the limit. Ordinary Read and Write calls pass through to
the same host. It does not prefetch, and cancellation cannot lose an unread tail.
Allocation uses `BytesMut`'s infallible allocation policy; the allocator may round
capacity up. Native chunk hosts do not need this adapter.

```rust
use nitori_io::{adapters::Chunked, bridge::Std, ChunkSegment, PollReadExt};
use std::{io::Cursor, num::NonZeroUsize, pin::Pin, task::{Context, Poll, Waker}};

let mut io = Chunked::new(
    Std::new(Cursor::new(b"hello")),
    NonZeroUsize::new(1024).unwrap(),
);
let mut cx = Context::from_waker(Waker::noop());
let Poll::Ready(Ok(Some(ChunkSegment { chunk, .. }))) = Pin::new(&mut io)
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

The query is `poll_remaining_len`, which can wait: a received stream knows
its final size only once the peer sends it. It fails with the source's
`LengthError` when the length cannot be known, such as after a reset;
memory sources report `!`. Inside an operation, `remaining_len()` awaits it.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{adapters::ReadLimit, prelude::*};

#[call(sync)]
async fn bounded_len<F: ExactLen<Length = usize>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<u8, F::LengthError> {
    io.compose(ReadLimit(4u8)).remaining_len().await
}
let mut input = &b"abc"[..];
assert_eq!(input.sync_bounded_len_unpin(), Ok(3u8));
assert_eq!(input, b"abc");
```

Slices, `Bytes`, `BytesMut`, `VecDeque<u8>`, and the standard-library
`Cursor` over byte arrays, slices, vectors, boxed slices, `Bytes`, or
`BytesMut` support the query. Seeking a cursor past its end gives zero.
`Chunked` forwards its inner source's exact length, with its `Length` and
`LengthError`.

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
forwards `Write` and `WriteLent`, lending at most what is left.

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
async fn write_bounded<F: Write<(), Report = ()>, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    payload: &[u8],
    room: usize,
) -> (usize, Result<(), WriteAllError<F::Error>>) {
    let mut bounded = io.compose(WriteLimit(room));
    let result = bounded.write_all(payload, ()).await;
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
`Count = usize`, hold no state, and forward `Read`, `ReadChunk`,
`ExactLen` and `Write` of the parent. There is no blanket implementation for readers, so
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

## Metadata and reports

Each `poll_read` reports one `ReadSegment { count, metadata }`: the bytes it
put into the destination and the `Read::Metadata` they share, such as whether
they arrived in 0-RTT. A segment has one metadata value, so a fill stops where
the attributes of the source's bytes change, leaving the destination short;
a chunk of `ReadChunk` likewise ends there, in a `ChunkSegment { chunk,
metadata }`. Each `poll_write` reports one `WriteSegment { count, report }`,
with what accepting the bytes produced as `Write::Report`; how the caller
wants the bytes written is the write options it passes (see
[Write options](#write-options)). `None` is no segment: EOF or no progress for reads, nothing accepted for
writes.

The associated types have no defaults. A host without attributes writes
`type Metadata = ()` or `type Report = ()`, and an adapter that forwards a
host names the host's types, so a forgotten forward fails to compile instead
of dropping the attributes. The operations made of several segments return
the segments' values merged with `Combine`, whose `Default` value is what
no segments merge to; `()` implements it. The chunk sequences yield each
`ChunkSegment` instead.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{Combine, calls::ReadExactError, prelude::*};

/// Whether some bytes arrived early.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Early(bool);
impl Combine for Early {
    fn combine(self, next: Self) -> Self {
        Early(self.0 || next.0)
    }
}

/// A request's bytes and whether some of them arrived early.
struct Received {
    bytes: [u8; 4],
    early: Early,
}

#[call(sync)]
async fn request<F: Read<Metadata = Early>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<Received, ReadExactError<F::Error>> {
    let mut bytes = [0; 4];
    let early = io.read_exact(&mut bytes.as_mut_slice(), 4).await?;
    Ok(Received { bytes, early })
}
```

Code that reads only from sources without metadata can read from one with
it through `adapters::MergedMetadata`: `io.compose(MergedMetadata::default())`
reports no metadata (`Metadata = ()`) and merges each segment's into the
state, which `into_state` returns. Chunk reads, skipping and the exact
length pass through; skipping reports no metadata, so that of skipped bytes
does not reach the state. Kept with a receiver in
a `ComposedReceiver`, the state merges every call on it.
`adapters::MergingSource` borrows any `nitori_call::ReceiverAccess` and
merges read metadata into a separately borrowed `MergedMetadata` state.
It provides the adapted access through `nitori_call::ProvideAccess`.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::{
    Combine,
    adapters::MergedMetadata,
    Value,
    calls::ReadArrayError,
    prelude::*,
};

/// Whether some bytes arrived early.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Early(bool);
impl Combine for Early {
    fn combine(self, next: Self) -> Self {
        Early(self.0 || next.0)
    }
}

/// Reads from sources without metadata only.
#[call(sync)]
async fn pair<F: Read<Metadata = ()>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<[u8; 2], ReadArrayError<F::Error>> {
    Ok(io.read_array::<2>().await?.value)
}

#[call(sync)]
async fn early_pair<F: Read<Metadata = Early>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<Value<[u8; 2], Early>, ReadArrayError<F::Error>> {
    let mut merging = io.compose(MergedMetadata::default());
    let value = pair(&mut merging).await?;
    Ok(Value {
        value,
        metadata: merging.into_state().0,
    })
}
```

## Write options

`Write<O>` is generic over the write options `O`: what the caller wants
done with a segment's bytes, such as the priority of a send or the path it
takes. Each `poll_write` borrows the segment's `&O`, and `Error`, `Report`
and, for lending, `Buffer` are chosen for each `O`, so a sink can implement
several options types, for example `Write<()>` beside `Write<O>` when `O`
has a default. Memory sinks and bridges take none: `Write<()>`. An adapter
that forwards a sink, such as a loan of a view, `Chunked`, a write budget
or `ReadSkip`, forwards every options type of the sink.

A written value carries its options as a `Value { value, metadata }`,
whose `metadata` is the options of its bytes, as it is the metadata of
the bytes a value was read from.

The composition state `adapters::MappedOptions(map)` writes through a sink
of `Write<O>` with other options: for a `map` that implements
`adapters::MapOptions<M, Options = O>`, it gives each segment the options
`map.map_options(&m)` lends for the `m` it is written with, making a sink of
`Write<M>`; a copy from a source with metadata `M` then chooses the options
of each segment from its metadata. The mapping lends options it keeps, so it
chooses among a few ready ones, as below, or lends a part of what it is
given; one that computes the options keeps the result in itself and lends
it.

```rust
#![feature(impl_trait_in_assoc_type)]
use bytes::Buf;
use nitori_call::{ComposedReceiver, family_receiver};
use nitori_io::{WriteSegment, adapters::{MapOptions, MappedOptions}, prelude::*};
use std::{num::NonZeroUsize, pin::Pin, task::{Context, Poll}};

/// How urgently bytes are to be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Priority(u8);

/// A sink that records each segment with its priority.
#[derive(Default)]
struct Sent(Vec<(Vec<u8>, Priority)>);
family_receiver!(impl [] for Sent);
impl Write<Priority> for Sent {
    type Error = !;
    type Report = ();
    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut B,
        priority: &Priority,
    ) -> Poll<Result<Option<WriteSegment<()>>, !>>
    where
        Self: 'v,
    {
        let bytes = input.copy_to_bytes(input.remaining()).to_vec();
        let count = NonZeroUsize::new(bytes.len());
        host.get_pin_mut().get_mut().0.push((bytes, *priority));
        Poll::Ready(Ok(count.map(|count| WriteSegment { count, report: () })))
    }
}

/// Whether bytes arrived early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Early(bool);

/// Lends one of two priorities by whether bytes arrived early.
struct ByArrival {
    early: Priority,
    late: Priority,
}
impl MapOptions<Early> for ByArrival {
    type Options = Priority;
    fn map_options<'a>(&'a mut self, early: &'a Early) -> &'a Priority {
        if early.0 { &self.early } else { &self.late }
    }
}

// Early bytes are sent first.
let urgency = ByArrival { early: Priority(0), late: Priority(7) };
let mut sink = ComposedReceiver::new(Sent::default(), MappedOptions(urgency));
sink.sync_write_all_unpin(&b"ab"[..], Early(true)).unwrap();
sink.sync_write_all_unpin(&b"c"[..], Early(false)).unwrap();
let (sent, _) = sink.into_parts();
assert_eq!(sent.0, [(b"ab".to_vec(), Priority(0)), (b"c".to_vec(), Priority(7))]);
```

With the mapping `adapters::FixedOptions(o)`, `MappedOptions(FixedOptions(o))`
gives every segment `o`, borrowed, whatever it is written with, so the composed sink implements `Write<M>` for every
`M`, `()` included: code that writes only to sinks without options, or a
whole copy, can then write to a sink that takes them. `MappedOptions`
composes on any family, inside an operation with `io.compose(..)` or kept
with a receiver in a `ComposedReceiver`, and forwards `Write` and
`WriteLent`. The mapping is called on every write of a segment, a write that
waits included; a fill's options are mapped when it returns `Filled`, not
when it gives up, and the sink
gets a clone of the options the mapping lends, so lending needs
`Options: Clone`.

```rust
#![feature(impl_trait_in_assoc_type)]
# use bytes::Buf;
# use nitori_call::family_receiver;
# use nitori_io::WriteSegment;
# use std::{num::NonZeroUsize, pin::Pin, task::{Context, Poll}};
# #[derive(Clone, Copy, Debug, PartialEq, Eq)]
# struct Priority(u8);
# #[derive(Default)]
# struct Sent(Vec<(Vec<u8>, Priority)>);
# family_receiver!(impl [] for Sent);
# impl Write<Priority> for Sent {
#     type Error = !;
#     type Report = ();
#     fn poll_write<'v, B: Buf + ?Sized>(
#         host: Pin<&mut Self::ReceiverView<'v>>,
#         _: &mut Context<'_>,
#         input: &mut B,
#         priority: &Priority,
#     ) -> Poll<Result<Option<WriteSegment<()>>, !>>
#     where
#         Self: 'v,
#     {
#         let bytes = input.copy_to_bytes(input.remaining()).to_vec();
#         let count = NonZeroUsize::new(bytes.len());
#         host.get_pin_mut().get_mut().0.push((bytes, *priority));
#         Poll::Ready(Ok(count.map(|count| WriteSegment { count, report: () })))
#     }
# }
use nitori_call::{ReceiverAccess, call};
use nitori_io::{
    adapters::{FixedOptions, MappedOptions},
    calls::WriteAllError,
    prelude::*,
};

/// Writes only to sinks without options.
#[call(sync)]
async fn greet<F: Write<(), Report = ()>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<(), WriteAllError<F::Error>> {
    io.write_all(&b"hello"[..], ()).await
}

#[call(sync)]
async fn urgent_greeting<F: Write<Priority, Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) {
    greet(&mut io.compose(MappedOptions(FixedOptions(Priority(0))))).await.unwrap();
}

let mut sink = Sent::default();
sink.sync_urgent_greeting_unpin();
assert_eq!(sink.0, [(b"hello".to_vec(), Priority(0))]);
```

## Lending storage

A sink that implements `WriteLent<O>` lends its own storage, so a source can
produce bytes straight into it. In one poll `poll_write_lent` grants between
one and `wanted` bytes and calls the fill with a `&mut LentBuffer` that holds
at most the granted count. The fill puts a prefix into `buffer()`, a
`bytes::buf::Limit` it can pass to `Read::poll_read`, and returns
`Filled { value, options }`: its value and the write options of the
bytes it put, which the sink writes them with if it keeps them. The grant
therefore does not depend on the options. The fill can also give up with
its own error, which writes nothing. The sink then counts
what it keeps from its own storage, not from what the fill reports, and
answers a `LentOutcome`: `Kept(LentSegment { count, report, value })` with at
least one byte, `NotKept(value)` when no byte was put or the loan was broken
by bypassing the limit, or `GaveUp(error)`. `None` means the sink accepts no
byte, like a full slice or a write budget at its boundary. The fill runs at
most once per poll and not at all on `Pending`; it cannot await, so a source
it reads from polls synchronously.

A sink lends with `LentBuffer::new` on storage that has room for the grant,
and takes the storage back with `into_storage` if it counts through it. A
sink that lends also implements `Write` with the same options; neither is
derived from the other.

```rust
use bytes::BufMut;
use nitori_io::{Filled, LentOutcome, LentSegment, PollWriteExt};
use std::{num::NonZeroUsize, pin::Pin, task::{Context, Poll, Waker}};

let mut sink = bytes::BytesMut::new();
let mut cx = Context::from_waker(Waker::noop());
let Poll::Ready(Ok(Some(LentOutcome::Kept(LentSegment { count, .. })))) = Pin::new(&mut sink)
    .poll_write_lent(&mut cx, NonZeroUsize::new(8).unwrap(), |lent| {
        lent.buffer().put_slice(b"abc");
        Ok::<_, !>(Filled { value: (), options: () })
    })
else { panic!("a memory sink lends at once") };
assert_eq!((count.get(), &sink[..]), (3, &b"abc"[..]));
```

## Copying

Three operations copy from a source to a sink until the source ends, chosen by
what the two implement, with no specialization:

- `calls::copy_chunks(source, sink, chunk_maximum)` takes chunks from a
  `ReadChunk` source and writes each completely; owned chunks such as `Bytes`
  can reach the sink without a copy.
- `calls::copy_into_lent(source, sink, chunk_maximum)` reads from a `Read`
  source straight into the storage a `WriteLent` sink lends, in the same
  poll. A source that has to wait gives the loan up, so cancellation loses
  nothing. When the sink accepts no byte, one byte is read to tell the
  source's end, which succeeds, from zero progress.
- `calls::copy_buffered(source, sink, buffer)` reads into a buffer of the
  caller's and writes it out, for a source and a sink with only `Read` and
  `Write`. An empty buffer is an `InsufficientCapacityError` before any IO.

A copy writes each segment with the metadata it was read with as its write
options: the source has `Metadata = M`, the sink implements `Write<M>`
(`WriteLent<M>` for `copy_into_lent`, which also needs `M: Clone` to merge
the metadata it gives the sink), and the copy takes no options of its
own. `copy_buffered` writes a segment completely before reading the next, so
bytes of different metadata are never written together. A copy from a
source without metadata into a sink without options is `()` to `()`. For
a sink that takes other options, compose `adapters::MappedOptions` on it,
with a `MapOptions` mapping to choose them from each segment's metadata, or
with `FixedOptions` to write the whole copy with one (see [Write options](#write-options)).

Each returns a `CopyTotal { count, metadata, report }` with the count as a
`u64` and the segments' metadata and reports merged with `Combine`. Their
errors carry the bytes copied before the failure; a sink that accepts nothing
is a `WriteZeroError<u64>`. To copy exactly `n` bytes, compose
`ReadLimit(n)` on the source and compare the count with `n`. Dropping
`copy_chunks` or `copy_buffered` while it writes loses the bytes it has taken
from the source and the sink has not accepted, and so does a write failure or
zero progress, which reports only the count.

They take two accesses, so they are called as functions inside operations or
through their `{Name}HostExt` on a pair of receivers.

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

`write::Write` borrows its input cursor and its options, and advances the
cursor by the accepted count.
`WriteAll` takes any `Buf` by value; pass `&mut buffer` to keep the cursor,
which after a failure or cancellation holds the unaccepted tail. Its errors
report the total accepted bytes. A write that accepts nothing of nonempty input,
`None`, is a `WriteZeroError` failure. Accepted bytes do not imply flushing or remote delivery.

Derived operations retain progress across Pending. Dropping a call does not
roll back earlier successful reads or writes. External borrowed cursors retain
their progress; owned inputs, partial arrays, and undelivered owned storage are
dropped with their owner. Cancellation has no ordinary completion result.

Empty basic Read/Write requests still reach the host, allowing terminal errors
to take precedence. Zero-length Exact/Array/Chunks and an empty WriteAll complete
locally without checking the host, with the default metadata or report. ReadToEnd with zero destination capacity
fails locally. Empty basic reads do not establish EOF.

## Chunks and conversion helpers

Chunk sequences yield one nonempty bounded chunk per poll and never collect
the whole sequence. Read an execution as a Stream to receive
`ControlFlow::Continue(segment)` for each `ChunkSegment`, then one `Break` with the final
value; awaiting it drops the chunks. Inside an operation, pin the child and
await `child.as_mut().next()` to relay its chunks and observe its final value.

`helpers::poll_read_from_chunk` copies a single bounded chunk into the destination,
reporting the chunk's metadata.
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

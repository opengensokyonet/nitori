# nitori_codec

Experimental encoding and decoding of values on the sources and sinks of
`nitori_io`. Requires Rust nightly.

## Values on sources and sinks

Implement `DecodeOn<S>` to read a value and `EncodeOn<S>` to write it.
Both methods borrow the access as `&mut S`. Implement them for any
`S: nitori_call::ReceiverAccess` whose family has the required IO
capabilities. This includes call slots and types that offer access through
`ProvideAccess`.

The framework supplies `DecodeFrom<S, How>` and `EncodeInto<S, How>` as
the entry points. It reports the start of each value through the access plan
to any attached `nitori_call::TrackValues` trackers. A value ends with a
completion report on success or an abandonment report on error or when
its in-progress future is dropped, even if it transferred no bytes. Each
tracker can use those reports and the IO it observed to decide how to handle
an abandonment. Decode and encode fields through `decode` and `encode`
or their `DecodeFrom` and `EncodeInto` entry points so they get their own
nested reports.

Inside an operation, `Decode` and `Encode` borrow an access through
`io.decode::<V>()` and `io.encode(value)`. On an executor such as a
memory buffer, `DecodeHostExt` and `EncodeHostExt` run the codec as the whole
operation of a call, with `sync_` versions for executors that never wait.
Use `nitori_codec::prelude::*` to import the codec and extension traits.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_codec::prelude::*;
use nitori_io::{Read, calls::ReadArrayError};

#[call(sync)]
async fn swap<F: Read<Metadata = ()>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<[u8; 2], ReadArrayError<F::Error>> {
    let [first] = io.decode::<[u8; 1]>().await?;
    let [second] = io.decode::<[u8; 1]>().await?;
    Ok([second, first])
}

let mut input = &b"abcd"[..];
assert_eq!(input.sync_swap_unpin().unwrap(), *b"ba");
assert_eq!(input.sync_decode_unpin::<[u8; 2]>().unwrap(), *b"cd");

let mut output = Vec::new();
output.sync_encode_unpin(*b"xy").unwrap();
output.sync_encode_unpin(&b"z"[..]).unwrap();
assert_eq!(output, b"xyz");
```

Byte arrays decode from a source and encode into a sink; slices and `Bytes`
encode. They write with the write options `()`, into sinks of
`Write<()>` (see [Write options](#write-options)). An encoding returns
the sink's reports of the bytes written, merged with `Combine`.

## Values that keep their IO

With `OnAccess`, the default mode, the framework takes an access by value
or as a mutable borrow. With `OnReceiver`, it owns the receiver for the call
that decodes the value or encodes its output. The compiler can infer the
mode from the IO argument.

`DecodeOn::Decoded` defaults to the value itself, and `EncodeOn::Encoded`
defaults to `EncodeOn::Output`. When the value keeps its source, set
`Decoded = KeepsIo<Self>`. When the output keeps its sink, set
`Encoded = KeepsIo<Self::Output>`. Build the value with `KeepsIo::new`,
which gives the closure a `Vacant` slot for the IO. The framework reports
completion before filling the slot with the access or receiver. With
`OnReceiver`, the call's built-in slot has no tracker attached.

Implement `staged::MapIo` for the value that holds the IO. The derive
supports structs and enums. Use `#[map_io(S)]` when `S` is the IO type
parameter, or `#[map_io(within = P)]` when a field of type `P: MapIo`
holds it. Each variant keeps the IO in one field. `take_io(value)`
returns `IoParts { value, io }` with a `Vacant` slot in `value` and the
extracted IO in `io`.
`MapIo::map_io` can fill that slot with another IO object.

```rust
use nitori_call::ReceiverAccess;
use nitori_codec::{prelude::*, staged::{IoParts, KeepsIo, MapIo, take_io}};
use nitori_io::{Read, calls::ReadArrayError};

#[derive(MapIo)]
#[map_io(S)]
struct Tagged<S> {
    tag: u8,
    source: S,
}

impl<S: ReceiverAccess<Family: Read<Metadata = ()>>> DecodeOn<S> for Tagged<S> {
    type Error = ReadArrayError<<S::Family as Read>::Error>;

    type Decoded = KeepsIo<Self>;

    async fn decode_on(source: &mut S) -> Result<Self::Decoded, Self::Error> {
        let [tag] = source.decode::<[u8; 1]>().await?;
        Ok(KeepsIo::new(|source| Tagged { tag, source }))
    }
}

async fn read_tag() {
    let tagged = Tagged::decode_from(b"abc".as_slice()).await.unwrap();
    let IoParts { value: tagged, io: mut source } = take_io(tagged);
    assert_eq!(tagged.tag, b'a');
    assert_eq!(source.sync_decode_unpin::<[u8; 2]>().unwrap(), *b"bc");
}
# nitori_call::__private::run_sync(read_tag());
```

## Optional values at the end of a source

`Option<T>` decodes from a source whose family implements `nitori_io::AtEnd`.
It queries the source without consuming a byte, returns `None` at the end,
and otherwise decodes `T` as a nested value. A source ending during `T`
is handled by `T`'s decoder. `option::DecodeOptionError` carries the
query error in `AtEnd { source }` or the decoder's error in
`Value { source }`.

On an access, a `T` that keeps its IO keeps it inside `Some`. When the
result is `None`, the access passed to the decoder is dropped. For a staged
`T` that keeps an owned receiver, use `T::decode_from` with `OnReceiver`.

```rust
use nitori_codec::prelude::*;

let mut input = b"x".as_slice();
assert_eq!(input.sync_decode_unpin::<Option<[u8; 1]>>().unwrap(), Some(*b"x"));
assert_eq!(input.sync_decode_unpin::<Option<[u8; 1]>>().unwrap(), None);
```

## Metadata

A source's reads carry metadata, such as whether the bytes arrived in
0-RTT, and decoded values keep it by their structure:

- A base value reads its own bytes and decodes no part through another
  value, such as a byte array here, or a variable-length or fixed-width
  integer of a protocol crate. It decodes only from sources without
  metadata (`Metadata = ()`). A value that holds nothing of its own, such
  as a marker whose value is a constant, counts as a base value even when
  it reads through another value's decoding, since it has no place for the
  metadata.
- `nitori_io::Value<V, M>` decodes `V` from a source with
  metadata `M` and holds `{ value, metadata }`, the merged metadata of the
  bytes `V` read. `V` reads through a `nitori_io::adapters::MergedMetadata`,
  which reports no metadata to it.
- A composite value decodes each part through that part's own decoding and
  reads no bytes itself. It takes the metadata type as a parameter `M`,
  `()` by default, and decodes from sources whose metadata is `M`. Each
  base field is a `Value<Field, M>`; a composite field takes the same
  `M` and is not wrapped again; a part kept to be read in stages, such as a
  payload over the source, reports the metadata of each later read itself.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_codec::prelude::*;
use nitori_io::{Combine, Read, Value, calls::ReadArrayError};

/// A composite value of two base fields.
struct Header<M = ()> {
    kind: Value<[u8; 1], M>,
    length: Value<[u8; 2], M>,
}
impl<S, M> DecodeOn<S> for Header<M>
where
    S: ReceiverAccess<Family: Read<Metadata = M>>,
    M: Combine + Unpin,
{
    type Error = ReadArrayError<<S::Family as Read>::Error>;
    async fn decode_on(io: &mut S) -> Result<Self, Self::Error> {
        Ok(Header {
            kind: io.decode::<Value<[u8; 1], M>>().await?,
            length: io.decode::<Value<[u8; 2], M>>().await?,
        })
    }
}

let mut input = &b"\x01\x00\x05"[..];
let header = input.sync_decode_unpin::<Header>().unwrap();
assert_eq!((header.kind.value, header.length.value), ([1], [0, 5]));
```

A base value does not decode from a source with metadata, which would drop
it:

```compile_fail,E0271
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_codec::prelude::*;
use nitori_io::{Read, calls::ReadArrayError};

#[derive(Default)]
struct Early(bool);
impl nitori_io::Combine for Early {
    fn combine(self, next: Self) -> Self {
        Early(self.0 || next.0)
    }
}

#[call(sync)]
async fn drops<F: Read<Metadata = Early>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<[u8; 2], ReadArrayError<F::Error>> {
    io.decode::<[u8; 2]>().await
}
```

## Write options

A sink's writes take options, the `O` of `nitori_io::Write<O>`, such as
how urgently the bytes are to be sent, and encoded values give them by the
same structure as metadata:

- A base value writes with the options `()`, into sinks of `Write<()>`.
- `Value<V, O>` encodes `V` into a sink of `Write<O>` with `metadata` as the
  options of every byte `V` writes. A sink wrapped with
  `nitori_io::adapters::MappedOptions::new(sink, FixedOptions(metadata))`
  accepts any options from `V` and passes the fixed options to the sink.
- A composite value takes the options type as its parameter `O` and
  encodes into sinks of `Write<O>`, each base field a `Value<Field, O>`, so
  the bytes of each field are written with that field's options.

A value decoded from a source with metadata `M` holds `M` where it would
hold options. Written into a sink that takes other options, or none,
wrap the sink in `nitori_io::adapters::MappedOptions`, whose
mapping, a `MapOptions`, lends the sink's options for each segment's, or
`FixedOptions` for one set for every segment. The encoded length
of a `Value` is its value's.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::ReceiverAccess;
use nitori_codec::prelude::*;
use nitori_io::{
    Combine, Value, Write,
    adapters::{FixedOptions, MappedOptions},
    calls::WriteArrayError,
};

/// A composite value of two base fields.
struct Header<O = ()> {
    kind: Value<[u8; 1], O>,
    length: Value<[u8; 2], O>,
}
impl<S, O> EncodeOn<S> for Header<O>
where
    S: ReceiverAccess<Family: Write<O, Report: Combine>>,
    O: Unpin,
{
    type Output = <S::Family as Write<O>>::Report;
    type Error = WriteArrayError<<S::Family as Write<O>>::Error>;
    async fn encode_on(self, io: &mut S) -> Result<Self::Output, Self::Error> {
        let kind = io.encode(self.kind).await?;
        Ok(kind.combine(io.encode(self.length).await?))
    }
}

/// How urgently bytes are to be sent.
#[derive(Clone, Copy)]
struct Priority(u8);

let header = Header {
    kind: Value { value: [1], metadata: Priority(0) },
    length: Value { value: [0, 5], metadata: Priority(3) },
};
// A vector takes no options: give it `()` for every segment.
let mut output = MappedOptions::new(Vec::new(), FixedOptions(()));
output.sync_encode_unpin(header).unwrap();
assert_eq!(output.into_parts().io, b"\x01\x00\x05");
```

## Encoded length

`EncodedLen` reports the byte length as its associated type `Length`.
Its future can wait for a source to learn its final size or return an error
if the source cannot tell. `KnownLen` extends `EncodedLen<LengthError = !>`
and reports the same length synchronously. Byte arrays, byte slices and
`Bytes` use `usize` and implement both traits. `Value<V, M>` uses its
inner value's length and implements the length traits available on `V`.

When a pair of constructors needs an encoded length, use the plain name
such as `new` or `raw` for the synchronous constructor taking a
`KnownLen` value. Use the `_async` suffix for the constructor taking an
`EncodedLen` value.

For a composite value, choose a length type that covers the format's range
and use `Widen` to convert field lengths into it without losing bits.
For example, `usize` field lengths can widen to `u64` on a 32-bit target
that needs to represent lengths larger than `usize`. Add the widened
lengths with saturation so the enclosing constructor can reject a result
above its format's limit.

```rust
use nitori_codec::{KnownLen, Widen};

let value = b"abc".as_slice();
let len: u64 = Widen::widen(value.known_len());
assert_eq!(len, 3);
```

## Sources as payloads

`payload` writes a source out as the bytes of a value, one wrapper for each
pairing of capabilities, as the copies of `nitori_io::calls` pair them:

- `Chunks { source, chunk_maximum }`: the source hands out owned chunks
  (`ReadChunk`) and the sink accepts bytes (`Write`); shared chunks such as
  `Bytes` reach the sink without being copied.
- `Fill { source, chunk_maximum }`: the sink lends its storage
  (`WriteLent`) and the source reads into it (`Read`). When the sink
  lends no storage, the source checks for EOF without consuming bytes
  (`AtEnd`).
- `Buffered { source, buffer }`: both only read and accept bytes, through
  the caller's buffer; an empty buffer fails.

Each copies until the source ends, writing every segment with the metadata
it was read with as its write options, so the sink takes the source's
metadata type, `()` for a source without metadata. Wrap a sink that takes
other options in `MappedOptions` with a `MapOptions` mapping or `FixedOptions`. Each returns the
copy's total: the count,
the merged metadata of what it read and the merged report of what the sink
accepted. Bound the source to the bytes it is to contribute, such as with
`ReadLimit`. A wrapper over a source with `ExactLen` implements
`EncodedLen` using the source's own length type and `RemainderError`.
Query these wrappers through `EncodedLen`, even when the source is in
memory. Choose an explicit wrapper to encode a bare source using the
capabilities of the source and sink.

```compile_fail,E0277
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_codec::prelude::*;
use nitori_io::{Read, Write};

#[call(sync)]
async fn write_bare_source<F: Read<Metadata = ()>, G: Write<()>>(
    source: &mut impl ReceiverAccess<Family = F>,
    sink: &mut impl ReceiverAccess<Family = G>,
) {
    let _ = sink.encode(source).await;
}
```

```rust
#![feature(impl_trait_in_assoc_type)]
use core::num::NonZeroUsize;
use nitori_call::{ReceiverAccess, call};
use nitori_codec::{payload::Chunks, prelude::*};
use nitori_io::{ReadChunk, Write, adapters::ReadLimit};

#[call(sync)]
async fn write_bounded<F, G>(
    source: &mut impl ReceiverAccess<Family = F>,
    output: &mut impl ReceiverAccess<Family = G>,
) -> u64
where
    F: ReadChunk<Metadata = (), Error = !>,
    G: Write<(), Report = (), Error = !>,
{
    output.encode(Chunks {
        source: ReadLimit::new(source, 7u8),
        chunk_maximum: NonZeroUsize::new(4).unwrap(),
    }).await.unwrap().count
}

let mut source = &b"payload and more"[..];
let mut output = Vec::new();
let count = (&mut source, &mut output).sync_write_bounded_unpin();
assert_eq!((count, &output[..]), (7, &b"payload"[..]));
assert_eq!(source, b" and more");
```

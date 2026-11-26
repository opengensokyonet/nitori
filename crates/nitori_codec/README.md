# nitori_codec

Experimental encoding and decoding of values on the sources and sinks of
`nitori_io`. Requires Rust nightly.

## Values on sources and sinks

A value implements `EncodeInto<S>` to write itself into a sink of type `S`
and `DecodeFrom<S>` to read itself from a source of type `S`. `S` is taken
by value or as `&mut`: a decoded value may keep its source, reading what
comes first and leaving the rest in the source it keeps, or read everything
it needs and keep nothing. The traits are implemented on the values, so a
crate implements them for its own values over any source it supports.

Sources and sinks are usually `nitori_call::AsAccess` values: every access
is one, and so is a type of the caller's that lends an access, such as a
payload that keeps its stream composed with what is left of it. The
implementations in this crate are for every `S: AsAccess` whose family has
the capabilities they need. Decoding from a borrowed source, as
`io.decode::<V>()` does, is `Send` when the source is.

Inside an operation, `Decode` and `Encode` turn the traits around into
methods of an access: `io.decode::<V>()` and `io.encode(value)`, which
borrow the access for the value. On an executor such as a memory buffer,
`DecodeHostExt` and `EncodeHostExt` run them as the whole operation of a
call, with `sync_` versions for executors that never wait. The value is
decoded from the slot of the call, so a value that keeps its source is
decoded inside an operation instead. `use nitori_codec::prelude::*` imports
the traits and the extension traits.

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
  bytes `V` read. `V` reads through a `nitori_io::adapters::MergingSource`,
  which reports no metadata to it.
- A composite value decodes each part through that part's own decoding and
  reads no bytes itself. It takes the metadata type as a parameter `M`,
  `()` by default, and decodes from sources whose metadata is `M`. Each
  base field is a `Value<Field, M>`; a composite field takes the same
  `M` and is not wrapped again; a part kept to be read in stages, such as a
  payload over the source, reports the metadata of each later read itself.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{AsAccess, ReceiverAccess, call};
use nitori_codec::prelude::*;
use nitori_io::{Combine, Read, Value, calls::ReadArrayError};

/// A composite value of two base fields.
struct Header<M = ()> {
    kind: Value<[u8; 1], M>,
    length: Value<[u8; 2], M>,
}
impl<S, M> DecodeFrom<S> for Header<M>
where
    S: AsAccess<Family: Read<Metadata = M>>,
    M: Combine + Unpin,
{
    type Error = ReadArrayError<<S::Family as Read>::Error>;
    async fn decode_from(mut io: S) -> Result<Self, Self::Error> {
        Ok(Header {
            kind: Value::decode_from(io.access()).await?,
            length: Value::decode_from(io.access()).await?,
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
  options of every byte `V` writes. `V` writes through the sink composed
  with `nitori_io::adapters::MappedOptions(FixedOptions(metadata))`, which
  takes any options, `()` included, and gives the sink these.
- A composite value takes the options type as its parameter `O` and
  encodes into sinks of `Write<O>`, each base field a `Value<Field, O>`, so
  the bytes of each field are written with that field's options.

A value decoded from a source with metadata `M` holds `M` where it would
hold options. Written into a sink that takes other options, or none,
the sink is composed with `nitori_io::adapters::MappedOptions`, whose
mapping, a `MapOptions`, lends the sink's options for each segment's, or
`FixedOptions` for one set for every segment. The encoded length
of a `Value` is its value's.

```rust
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{AsAccess, ComposedReceiver};
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
impl<S, O> EncodeInto<S> for Header<O>
where
    S: AsAccess<Family: Write<O, Report: Combine>>,
    O: Unpin,
{
    type Output = <S::Family as Write<O>>::Report;
    type Error = WriteArrayError<<S::Family as Write<O>>::Error>;
    async fn encode_into(self, mut io: S) -> Result<Self::Output, Self::Error> {
        let kind = self.kind.encode_into(io.access()).await?;
        Ok(kind.combine(self.length.encode_into(io.access()).await?))
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
let mut output = ComposedReceiver::new(Vec::new(), MappedOptions(FixedOptions(())));
output.sync_encode_unpin(header).unwrap();
assert_eq!(output.into_parts().0, b"\x01\x00\x05");
```

## Encoded length

`EncodedLen` tells how many bytes a value encodes to, as a `u64`, for
formats that write the length first. Telling it can wait and can fail, as
for a value whose bytes come from a stream that learns its final size later
or is reset first. Byte arrays, slices and `Bytes` always know it.

## Sources as payloads

`payload` writes a source out as the bytes of a value, one wrapper for each
pairing of capabilities, as the copies of `nitori_io::calls` pair them:

- `Chunks { source, chunk_maximum }`: the source hands out owned chunks
  (`ReadChunk`) and the sink accepts bytes (`Write`); shared chunks such as
  `Bytes` reach the sink without being copied.
- `Fill { source, chunk_maximum }`: the sink lends its storage
  (`WriteLent`) and the source reads into it (`Read`).
- `Buffered { source, buffer }`: both only read and accept bytes, through
  the caller's buffer; an empty buffer fails.

Each copies until the source ends, writing every segment with the metadata
it was read with as its write options, so the sink takes the source's
metadata type, `()` for a source without; compose `MappedOptions`, with a
`MapOptions` mapping or `FixedOptions`, on a sink that takes other options. Each returns the
copy's total: the count,
the merged metadata of what it read and the merged report of what the sink
accepted. Bound the source to the bytes it is to contribute, such as with
`ReadLimit`. Its `EncodedLen` is the source's exact remaining length
(`ExactLen`), which fails when the source cannot tell it or it does not fit
in 64 bits. A bare source is not a value: which pairing it would take
cannot be told from it, and a crate that keeps a source in a type of its own
encodes that type as it chooses.

```compile_fail,E0271
use nitori_call::ConstructedAccess;
use nitori_codec::prelude::*;

let mut output = Vec::new();
output.sync_encode_unpin(ConstructedAccess::new(&b"payload"[..])).unwrap();
```

```rust
use core::num::NonZeroUsize;
use nitori_call::{Composed, ConstructedAccess};
use nitori_codec::{payload::Chunks, prelude::*};
use nitori_io::adapters::ReadLimit;

let mut source = &b"payload and more"[..];
let mut output = Vec::new();
let total = output
    .sync_encode_unpin(Chunks {
        source: Composed::new(ConstructedAccess::new(&mut source), ReadLimit(7u8)),
        chunk_maximum: NonZeroUsize::new(4).unwrap(),
    })
    .unwrap();
assert_eq!((total.count, &output[..]), (7, &b"payload"[..]));
assert_eq!(source, b" and more");
```

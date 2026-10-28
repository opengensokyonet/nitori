//! Operations whose access names its family with a type parameter.
#![feature(impl_trait_in_assoc_type, yield_expr)]
use futures_core::Stream;
use nitori_call::{ReceiverAccess, call};
use nitori_io::{
    calls::{ReadChunksExactError, WriteAllError},
    prelude::*,
};
use std::{
    num::NonZeroUsize,
    ops::ControlFlow,
    pin::pin,
    task::{Context, Poll, Waker},
};

#[call(sync)]
async fn write_twice<F: Write, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    bytes: &[u8],
) -> Result<(), WriteAllError<F::Error>> {
    io.write_all(bytes).await?;
    io.write_all(bytes).await
}

/// The family bounded in the where clause, with an anonymous access, calling
/// the operation above both as a function and as a method.
#[call(sync)]
async fn write_four<F>(
    io: &mut impl ReceiverAccess<Family = F>,
    bytes: &[u8],
) -> Result<(), WriteAllError<F::Error>>
where
    F: Write,
{
    write_twice(io, bytes).await?;
    io.write_twice(bytes).await
}

/// An operation with a family bound calling one with a named family.
#[call(sync)]
async fn write_eight<A: ReceiverAccess<Family: Write>>(
    io: &mut A,
    bytes: &[u8],
) -> Result<(), WriteAllError<<A::Family as Write>::Error>> {
    io.write_four(bytes).await?;
    io.write_four(bytes).await
}

#[test]
fn named_families_compose_with_bounded_ones() {
    let mut output = Vec::new();
    output.sync_write_eight_unpin(b"ab").unwrap();
    assert_eq!(output, b"ab".repeat(8));
}

#[call(yields = F::Chunk)]
async fn chunks<F: ReadChunk, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    length: usize,
) -> Result<(), ReadChunksExactError<<F as Read>::Error>> {
    let mut chunks = pin!(io.read_chunks_exact(length, NonZeroUsize::new(2).unwrap()));
    loop {
        match chunks.as_mut().step().await {
            ControlFlow::Continue(chunk) => yield chunk,
            ControlFlow::Break(result) => return result,
        }
    }
}

#[test]
fn items_name_the_family_by_its_parameter() {
    let mut input = &b"abcde"[..];
    let mut execution = pin!(input.chunks_unpin(5));
    let mut cx = Context::from_waker(Waker::noop());
    let mut items = Vec::new();
    while let Poll::Ready(Some(event)) = execution.as_mut().poll_next(&mut cx) {
        match event {
            ControlFlow::Continue(chunk) => items.push(chunk.to_vec()),
            ControlFlow::Break(result) => result.unwrap(),
        }
    }
    assert_eq!(items, [&b"ab"[..], b"cd", b"e"]);
}

/// An argument whose type depends on the family.
#[call(sync)]
async fn chunk_length<F: ReadChunk, A: ReceiverAccess<Family = F>>(
    _io: &mut A,
    chunk: Option<<F as ReadChunk>::Chunk>,
) -> usize {
    chunk.map_or(0, |chunk| bytes::Buf::remaining(&chunk))
}

#[test]
fn arguments_may_depend_on_a_named_family() {
    let mut input = &b""[..];
    assert_eq!(input.sync_chunk_length_unpin(Some(&b"xy"[..])), 2);
    assert_eq!(input.sync_chunk_length_unpin(None), 0);
}

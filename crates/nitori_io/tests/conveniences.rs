#![feature(impl_trait_in_assoc_type, yield_expr)]
#![allow(clippy::type_complexity)] // results paired with the remaining budget
//! The prelude, the error aliases, the byte budgets, the endian writes, and
//! reading a child's events with `step`.
use bytes::{Buf, BufMut, Bytes};
use nitori_call::{ReceiverAccess, call, family_receiver};
use nitori_io::{
    adapters::{ReadLimit, WriteLimit},
    calls::{ReadArrayError, ReadChunksExactError, WriteAllError, WriteLeError},
    prelude::*,
};
use std::{
    num::NonZeroUsize,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll},
};

/// A host that has failed: every request reports the failure, even an empty one.
struct Closed;
family_receiver!(impl [] for Closed);
impl Read for Closed {
    type Error = &'static str;
    fn poll_read<'v, B: BufMut + ?Sized>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        _: &mut B,
    ) -> Poll<Result<usize, &'static str>>
    where
        Self: 'v,
    {
        Poll::Ready(Err("closed"))
    }
}
impl Write for Closed {
    type Error = &'static str;
    fn poll_write<'v, B: Buf + ?Sized>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        _: &mut B,
    ) -> Poll<Result<usize, &'static str>>
    where
        Self: 'v,
    {
        Poll::Ready(Err("closed"))
    }
}

#[call(sync)]
async fn read_limited<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
    budget: usize,
    output: &mut Vec<u8>,
) -> (usize, Result<usize, <A::Family as Read>::Error>) {
    let mut limited = io.compose(ReadLimit(budget));
    let mut total = 0;
    let result = loop {
        match limited.read(&mut *output).await {
            Ok(0) => break Ok(total),
            Ok(count) => total += count,
            Err(error) => break Err(error),
        }
    };
    (limited.into_state().0, result)
}

#[test]
fn read_limit_ends_at_the_boundary_and_keeps_the_rest() {
    let mut input = b"abcdef".as_slice();
    let mut output = Vec::new();
    let (left, result) = input.sync_read_limited_unpin(4, &mut output);
    assert_eq!((left, result), (0, Ok(4)));
    assert_eq!(output, b"abcd");
    assert_eq!(input, b"ef");
}

#[test]
fn read_limit_reports_what_remains_at_early_eof() {
    let mut input = b"ab".as_slice();
    let mut output = Vec::new();
    let (left, result) = input.sync_read_limited_unpin(5, &mut output);
    assert_eq!((left, result), (3, Ok(2)));
}

#[test]
fn read_limit_answers_eof_at_the_boundary_without_the_parent() {
    let mut output = Vec::new();
    let (left, result) = Closed.sync_read_limited_unpin(0, &mut output);
    assert_eq!((left, result), (0, Ok(0)));
    // Within the budget the parent is polled and its failure reported.
    let (left, result) = Closed.sync_read_limited_unpin(1, &mut output);
    assert_eq!((left, result), (1, Err("closed")));
}

#[call(sync, yields = Bytes)]
async fn chunks_limited<A: ReceiverAccess<Family: ReadChunk<Chunk = Bytes>>>(
    io: &mut A,
    budget: usize,
    length: usize,
) -> Result<(), ReadChunksExactError<<A::Family as Read>::Error>> {
    let mut limited = io.compose(ReadLimit(budget));
    let mut chunks = pin!(limited.read_chunks_exact(length, NonZeroUsize::new(4).unwrap()));
    loop {
        match chunks.as_mut().step().await {
            ControlFlow::Continue(chunk) => yield chunk,
            ControlFlow::Break(result) => return result,
        }
    }
}

#[test]
fn read_limit_caps_chunks() {
    use futures_core::Stream;
    let mut input = Bytes::from_static(b"abcdefgh");
    let mut execution = pin!(input.chunks_limited_unpin(6, 6));
    let mut cx = Context::from_waker(std::task::Waker::noop());
    let mut chunks = Vec::new();
    let result = loop {
        match execution.as_mut().poll_next(&mut cx) {
            Poll::Ready(Some(ControlFlow::Continue(chunk))) => chunks.push(chunk),
            Poll::Ready(Some(ControlFlow::Break(result))) => break result,
            other => panic!("memory reads complete at once: {:?}", other.is_ready()),
        }
    };
    result.unwrap();
    assert_eq!(chunks, [&b"abcd"[..], &b"ef"[..]]);
    // The budget ended the stream: a longer request is incomplete there.
    let mut input = Bytes::from_static(b"abcdefgh");
    let error = input.sync_chunks_limited_unpin(3, 5).unwrap_err();
    assert_eq!(error.completed(), 3);
    assert_eq!(input, &b"defgh"[..]);
}

#[call(sync)]
async fn write_limited<A: ReceiverAccess<Family: Write>>(
    io: &mut A,
    budget: usize,
    payload: &[u8],
) -> (
    usize,
    Result<(), WriteAllError<<A::Family as Write>::Error>>,
) {
    let mut limited = io.compose(WriteLimit(budget));
    let result = limited.write_all(payload).await;
    (limited.into_state().0, result)
}

#[test]
fn write_limit_accepts_the_budget_then_nothing() {
    let mut output = Vec::new();
    let (left, result) = output.sync_write_limited_unpin(3, b"ab");
    assert_eq!(left, 1);
    result.unwrap();
    let (left, result) = output.sync_write_limited_unpin(1, b"cd");
    assert_eq!(left, 0);
    let error = result.unwrap_err();
    assert!(matches!(error, WriteAllError::WriteZero { .. }));
    assert_eq!(error.completed(), 1);
    assert_eq!(output, b"abc");
}

#[test]
fn write_limit_accepts_nothing_at_the_boundary_without_the_parent() {
    let (left, result) = Closed.sync_write_limited_unpin(0, b"x");
    assert_eq!(left, 0);
    assert!(matches!(
        result.unwrap_err(),
        WriteAllError::WriteZero { .. }
    ));
    let (left, result) = Closed.sync_write_limited_unpin(1, b"x");
    assert_eq!(left, 1);
    assert_eq!(result.unwrap_err().host_error(), Some(&"closed"));
}

#[call(sync)]
async fn write_header<A: ReceiverAccess<Family: Write>>(io: &mut A) -> Result<(), usize> {
    io.write_array([0xff])
        .await
        .map_err(|error| error.completed())?;
    io.write_le(0x0102_u16)
        .await
        .map_err(|error| error.completed())?;
    io.write_be(0x0304_u16)
        .await
        .map_err(|error| error.completed())
}

#[test]
fn endian_writes_round_trip() {
    let mut output = Vec::new();
    output.sync_write_header_unpin().unwrap();
    assert_eq!(output, [0xff, 0x02, 0x01, 0x03, 0x04]);
    let mut input = &output[1..];
    assert_eq!(input.sync_read_le_unpin::<u16>().unwrap(), 0x0102);
    assert_eq!(input.sync_read_be_unpin::<u16>().unwrap(), 0x0304);
    let mut output = Vec::new();
    for value in [-1.5_f64, f64::MIN_POSITIVE] {
        output.sync_write_be_unpin(value).unwrap();
    }
    let mut input2 = output.as_slice();
    assert_eq!(input2.sync_read_be_unpin::<f64>().unwrap(), -1.5);
    assert_eq!(
        input2.sync_read_be_unpin::<f64>().unwrap(),
        f64::MIN_POSITIVE
    );
}

#[test]
fn endian_write_reports_progress_when_the_destination_fills() {
    let mut storage = [0_u8; 3];
    let mut output = storage.as_mut_slice();
    let error = output.sync_write_le_unpin(0x0102_0304_u32).unwrap_err();
    assert!(matches!(error, WriteLeError::WriteZero { .. }));
    assert_eq!(error.completed(), 3);
    assert_eq!(storage, [0x04, 0x03, 0x02]);
}

#[call(yields = u8)]
async fn bytes_then_count<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
) -> Result<usize, ReadArrayError<<A::Family as Read>::Error>> {
    let [a, b] = io.read_array::<2>().await?;
    yield a;
    yield b;
    Ok(2)
}

#[call(sync)]
async fn step_after_final<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
    events: &mut Vec<ControlFlow<usize, u8>>,
    past_the_end: bool,
) {
    let mut child = pin!(io.bytes_then_count());
    loop {
        let event = child.as_mut().step().await;
        let done = event.is_break();
        events.push(event.map_break(|result| result.unwrap_or(0)));
        if done {
            break;
        }
    }
    if past_the_end {
        let _ = child.as_mut().step().await;
    }
}

#[test]
fn step_gives_items_then_the_final_value() {
    let mut input = b"xy".as_slice();
    let mut events = Vec::new();
    input.sync_step_after_final_unpin(&mut events, false);
    assert_eq!(
        events,
        [
            ControlFlow::Continue(b'x'),
            ControlFlow::Continue(b'y'),
            ControlFlow::Break(2)
        ]
    );
}

#[test]
#[should_panic = "`step` after the final value"]
fn a_step_after_the_final_value_panics() {
    let mut input = b"xy".as_slice();
    input.sync_step_after_final_unpin(&mut Vec::new(), true);
}

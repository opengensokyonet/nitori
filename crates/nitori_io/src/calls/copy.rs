//! Copies from a source to a sink until the source ends, one operation for
//! each pairing of their capabilities.
//!
//! Choose by what the two sides implement: [`copy_chunks`] when the source
//! hands out owned chunks ([`ReadChunk`](crate::ReadChunk)),
//! [`copy_into_lent`] when the sink lends its storage
//! ([`WriteLent`](crate::WriteLent)) and the source supports [`AtEnd`],
//! and [`copy_buffered`], through a buffer of the caller's, when the two
//! sides only [`Read`](crate::Read) and [`Write`](crate::Write). To copy
//! exactly `n` bytes, wrap the source in
//! [`ReadLimit`](crate::adapters::ReadLimit) and compare the copied count
//! with `n`.
// The generated operations name both families' associated types through
// their accesses.
#![allow(clippy::type_complexity)]
use crate::{
    AtEnd, AtEndExt as _, ChunkSegment, Combine, Filled, LentOutcome, LentSegment,
    Read as ReadHost, ReadChunk as ChunkHost, ReadChunkExt as _, ReadExt as _, ReadSegment,
    Write as WriteHost, WriteExt as _, WriteLent, WriteSegment,
    error::{self, InsufficientCapacityError, WriteZeroError},
};
use bytes::Buf;
use core::{
    future::poll_fn,
    num::NonZeroUsize,
    task::{Context, Poll},
};
use nitori_call::{ReceiverAccess, call};
use snafu::{IntoError, Snafu};

/// Everything a copy moved: the bytes the sink accepted, the combined
/// metadata of the bytes read, and the combined report of the segments
/// written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CopyTotal<M, R> {
    /// The bytes copied, zero for a source that was already at its end.
    pub count: u64,
    /// The combined metadata of the bytes read.
    pub metadata: M,
    /// The combined report of the segments the sink accepted.
    pub report: R,
}

/// Add a segment's count to a copied total.
fn add(completed: u64, count: NonZeroUsize) -> u64 {
    // invariant: a usize fits in a u64 on every supported target, and a
    // u64 count of copied bytes does not overflow.
    u64::try_from(count.get())
        .ok()
        .and_then(|count| completed.checked_add(count))
        .expect("copied byte count overflow")
}

/// Write all of `input` to `sink` with the write options `options`,
/// adding what it accepts to the totals.
///
/// Returns the sink's failure or zero progress with the count copied by then.
async fn write_through<M, W: WriteHost<M, Report: Combine>>(
    sink: &mut impl ReceiverAccess<Family = W>,
    mut input: impl Buf,
    options: &M,
    completed: &mut u64,
    report: &mut W::Report,
) -> Result<(), WriteStepError<W::Error>> {
    while input.has_remaining() {
        match sink.write(&mut input, options).await {
            Ok(Some(WriteSegment {
                count,
                report: next,
            })) => {
                *completed = add(*completed, count);
                *report = core::mem::take(report).combine(next);
            }
            Ok(None) => {
                return Err(WriteStepError::WriteZero(
                    error::WriteZeroSnafu {
                        completed: *completed,
                    }
                    .build(),
                ));
            }
            Err(source) => return Err(WriteStepError::Sink(source)),
        }
    }
    Ok(())
}

/// How writing one piece of a copy failed.
enum WriteStepError<E> {
    Sink(E),
    WriteZero(WriteZeroError<u64>),
}

/// Copy until the source ends, taking chunks of at most `chunk_maximum`
/// bytes and writing each completely, with its metadata as the write
/// options, before taking the next.
///
/// Dropping the operation while it writes a chunk loses the part of the
/// chunk the sink has not accepted: it is taken from the source already.
#[call(sync)]
pub async fn copy_chunks<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    chunk_maximum: NonZeroUsize,
) -> Result<CopyTotal<R::Metadata, W::Report>, CopyChunksError<R::Error, W::Error>>
where
    R: ChunkHost,
    <R as ReadHost>::Metadata: Combine,
    W: WriteHost<R::Metadata>,
    <W as WriteHost<R::Metadata>>::Report: Combine,
{
    let mut completed = 0;
    let mut metadata = <R as ReadHost>::Metadata::default();
    let mut report = <W as WriteHost<R::Metadata>>::Report::default();
    loop {
        let segment = match source.read_chunk(chunk_maximum).await {
            Ok(Some(segment)) => segment,
            Ok(None) => break,
            Err(source) => {
                return Err(copy_chunks_error::ReadSnafu { completed }.into_error(source));
            }
        };
        let ChunkSegment {
            chunk,
            metadata: next,
        } = segment;
        match write_through(sink, chunk, &next, &mut completed, &mut report).await {
            Ok(()) => metadata = metadata.combine(next),
            Err(WriteStepError::Sink(source)) => {
                return Err(copy_chunks_error::WriteSnafu { completed }.into_error(source));
            }
            Err(WriteStepError::WriteZero(source)) => return Err(source.into()),
        }
    }
    Ok(CopyTotal {
        count: completed,
        metadata,
        report,
    })
}

/// Failures that [`copy_chunks`] can produce. Each carries the bytes copied
/// before it.
#[derive(Debug, Snafu)]
#[snafu(module(copy_chunks_error), visibility(pub(crate)))]
pub enum CopyChunksError<R, W> {
    /// The source failed.
    #[snafu(display("read failed after {completed} bytes were copied"))]
    Read { source: R, completed: u64 },
    /// The sink failed.
    #[snafu(display("write failed after {completed} bytes were copied"))]
    Write { source: W, completed: u64 },
    /// The sink accepted no byte of nonempty input.
    #[snafu(transparent)]
    WriteZero { source: WriteZeroError<u64> },
}

impl<R, W> CopyChunksError<R, W> {
    /// Bytes copied before the failure.
    pub fn completed(&self) -> u64 {
        match self {
            Self::Read { completed, .. } | Self::Write { completed, .. } => *completed,
            Self::WriteZero { source } => source.completed,
        }
    }
}

/// What ended one fill of a lent buffer without keeping bytes.
enum Stop<E> {
    /// The source has to wait; it registered the waker.
    Pending,
    /// The source is at its end.
    End,
    /// The source failed.
    Failed(E),
}

/// What one poll of [`copy_into_lent`] did.
enum LentStep<M, Rp, Re, We> {
    Kept(LentSegment<Rp, M>),
    /// The source reported bytes the sink did not keep.
    NotKept,
    End,
    ReadFailed(Re),
    WriteFailed(We),
    Full,
}

/// Copy until the source ends by reading straight into the sink's lent
/// storage, at most `chunk_maximum` bytes at a time, without a buffer in
/// between. The fill gives the sink the metadata of the segment it read as
/// its write options; a copy of it is merged into the total.
///
/// To transfer bytes, the copy asks the sink to lend and reads into the
/// buffer in the same poll. If the read has to wait, it gives up the loan
/// without consuming or keeping any bytes. Dropping the copy during this
/// wait loses no bytes.
///
/// When the sink lends no storage, [`AtEnd`] checks the source without
/// consuming bytes. Normal EOF completes the copy, an available byte
/// gives a zero-progress error, and a failed query returns
/// [`CopyIntoLentError::AtEnd`].
/// The sink counts what it keeps, so a source that reports bytes it did not
/// put into the lent buffer is zero progress.
#[call(sync)]
pub async fn copy_into_lent<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    chunk_maximum: NonZeroUsize,
) -> Result<
    CopyTotal<R::Metadata, W::Report>,
    CopyIntoLentError<R::Error, W::Error, <R as AtEnd>::RemainderError>,
>
where
    R: ReadHost + AtEnd,
    <R as ReadHost>::Metadata: Combine + Clone,
    W: WriteLent<R::Metadata>,
    <W as WriteHost<R::Metadata>>::Report: Combine,
{
    let mut completed = 0;
    let mut metadata = <R as ReadHost>::Metadata::default();
    let mut report = <W as WriteHost<R::Metadata>>::Report::default();
    loop {
        let step = poll_fn(|cx| {
            sink.poll_access(cx, |sink_view, cx| {
                // The sink and the source each get a context for the same
                // waker: the source is polled inside the sink's poll.
                let waker = cx.waker();
                let mut write_cx = Context::from_waker(waker);
                let mut read_cx = Context::from_waker(waker);
                let outcome = W::poll_write_lent(
                    sink_view.into_pin(),
                    &mut write_cx,
                    chunk_maximum,
                    |lent| {
                        let read = source.poll_access(&mut read_cx, |source_view, cx| {
                            R::poll_read(source_view.into_pin(), cx, lent.buffer())
                        });
                        match read {
                            Poll::Pending => Err(Stop::Pending),
                            Poll::Ready(Err(error)) => Err(Stop::Failed(error)),
                            // The lent buffer has room, so no segment is EOF.
                            Poll::Ready(Ok(None)) => Err(Stop::End),
                            Poll::Ready(Ok(Some(ReadSegment { metadata, .. }))) => Ok(Filled {
                                value: metadata.clone(),
                                options: metadata,
                            }),
                        }
                    },
                );
                match outcome {
                    Poll::Pending | Poll::Ready(Ok(Some(LentOutcome::GaveUp(Stop::Pending)))) => {
                        Poll::Pending
                    }
                    Poll::Ready(Err(error)) => Poll::Ready(LentStep::WriteFailed(error)),
                    Poll::Ready(Ok(None)) => Poll::Ready(LentStep::Full),
                    Poll::Ready(Ok(Some(LentOutcome::GaveUp(Stop::End)))) => {
                        Poll::Ready(LentStep::End)
                    }
                    Poll::Ready(Ok(Some(LentOutcome::GaveUp(Stop::Failed(error))))) => {
                        Poll::Ready(LentStep::ReadFailed(error))
                    }
                    Poll::Ready(Ok(Some(LentOutcome::NotKept(_)))) => {
                        Poll::Ready(LentStep::NotKept)
                    }
                    Poll::Ready(Ok(Some(LentOutcome::Kept(segment)))) => {
                        Poll::Ready(LentStep::Kept(segment))
                    }
                }
            })
        })
        .await;
        match step {
            LentStep::Kept(LentSegment {
                count,
                report: next_report,
                value: next_metadata,
            }) => {
                completed = add(completed, count);
                metadata = metadata.combine(next_metadata);
                report = report.combine(next_report);
            }
            LentStep::End => break,
            LentStep::NotKept => {
                return Err(error::WriteZeroSnafu { completed }.build().into());
            }
            LentStep::ReadFailed(source) => {
                return Err(copy_into_lent_error::ReadSnafu { completed }.into_error(source));
            }
            LentStep::WriteFailed(source) => {
                return Err(copy_into_lent_error::WriteSnafu { completed }.into_error(source));
            }
            LentStep::Full => match source.at_end().await {
                Ok(true) => break,
                Ok(false) => {
                    return Err(error::WriteZeroSnafu { completed }.build().into());
                }
                Err(source) => {
                    return Err(copy_into_lent_error::AtEndSnafu { completed }.into_error(source));
                }
            },
        }
    }
    Ok(CopyTotal {
        count: completed,
        metadata,
        report,
    })
}

/// Failures that [`copy_into_lent`] can produce, from a source failing with
/// `R`, a sink failing with `W`, and the source's
/// [`RemainderError`](AtEnd::RemainderError) `E`. Each carries the bytes
/// copied before it.
#[derive(Debug, Snafu)]
#[snafu(module(copy_into_lent_error), visibility(pub(crate)))]
pub enum CopyIntoLentError<R, W, E> {
    /// The source failed.
    #[snafu(display("read failed after {completed} bytes were copied"))]
    Read { source: R, completed: u64 },
    /// The sink failed.
    #[snafu(display("write failed after {completed} bytes were copied"))]
    Write { source: W, completed: u64 },
    /// The source could not tell whether it had reached EOF.
    #[snafu(display("end-of-source query failed after {completed} bytes were copied"))]
    AtEnd { source: E, completed: u64 },
    /// The sink lent nothing while a byte was available, or kept nothing
    /// from a fill that reported bytes.
    #[snafu(transparent)]
    WriteZero { source: WriteZeroError<u64> },
}

impl<R, W, E> CopyIntoLentError<R, W, E> {
    /// Bytes copied before the failure.
    pub fn completed(&self) -> u64 {
        match self {
            Self::Read { completed, .. }
            | Self::Write { completed, .. }
            | Self::AtEnd { completed, .. } => *completed,
            Self::WriteZero { source } => source.completed,
        }
    }
}

/// Copy until the source ends through `buffer`: read a segment into it, then
/// write it completely, with its metadata as the write options, before
/// reading again, so segments of different metadata are never written
/// together. The caller chooses the buffer's size and where it lives; it
/// holds the bytes read while the sink waits.
///
/// Dropping the operation while it writes loses the bytes read into the
/// buffer that the sink has not accepted.
#[call(sync)]
pub async fn copy_buffered<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    buffer: &mut [u8],
) -> Result<CopyTotal<R::Metadata, W::Report>, CopyBufferedError<R::Error, W::Error>>
where
    R: ReadHost,
    <R as ReadHost>::Metadata: Combine,
    W: WriteHost<R::Metadata>,
    <W as WriteHost<R::Metadata>>::Report: Combine,
{
    if buffer.is_empty() {
        return Err(error::InsufficientCapacitySnafu {
            required: 1usize,
            available: 0usize,
        }
        .build()
        .into());
    }
    let mut completed = 0;
    let mut metadata = <R as ReadHost>::Metadata::default();
    let mut report = <W as WriteHost<R::Metadata>>::Report::default();
    loop {
        let mut destination = &mut buffer[..];
        let (count, next) = match source.read(&mut destination).await {
            Ok(Some(ReadSegment {
                count,
                metadata: next,
            })) => (count, next),
            Ok(None) => break,
            Err(source) => {
                return Err(copy_buffered_error::ReadSnafu { completed }.into_error(source));
            }
        };
        let read = &buffer[..count.get()];
        match write_through(sink, read, &next, &mut completed, &mut report).await {
            Ok(()) => metadata = metadata.combine(next),
            Err(WriteStepError::Sink(source)) => {
                return Err(copy_buffered_error::WriteSnafu { completed }.into_error(source));
            }
            Err(WriteStepError::WriteZero(source)) => return Err(source.into()),
        }
    }
    Ok(CopyTotal {
        count: completed,
        metadata,
        report,
    })
}

/// Failures that [`copy_buffered`] can produce. Each carries the bytes
/// copied before it.
#[derive(Debug, Snafu)]
#[snafu(module(copy_buffered_error), visibility(pub(crate)))]
pub enum CopyBufferedError<R, W> {
    /// The buffer is empty, so nothing could be read; reported before any
    /// IO.
    #[snafu(transparent)]
    InsufficientCapacity { source: InsufficientCapacityError },
    /// The source failed.
    #[snafu(display("read failed after {completed} bytes were copied"))]
    Read { source: R, completed: u64 },
    /// The sink failed.
    #[snafu(display("write failed after {completed} bytes were copied"))]
    Write { source: W, completed: u64 },
    /// The sink accepted no byte of nonempty input.
    #[snafu(transparent)]
    WriteZero { source: WriteZeroError<u64> },
}

impl<R, W> CopyBufferedError<R, W> {
    /// Bytes copied before the failure.
    pub fn completed(&self) -> u64 {
        match self {
            Self::InsufficientCapacity { .. } => 0,
            Self::Read { completed, .. } | Self::Write { completed, .. } => *completed,
            Self::WriteZero { source } => source.completed,
        }
    }
}

//! The skip operations: an exact count, reporting what was left at EOF or on
//! failure, and skipping to EOF.
#![feature(impl_trait_in_assoc_type)]
use bytes::{BufMut, Bytes};
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{ComposedReceiver, ReceiverAccess, call};
use nitori_io::{
    Infinite,
    adapters::{ReadLimit, ReadSkip},
    calls::SkipExactError,
    prelude::*,
};

#[test]
fn skips_an_exact_count() {
    let mut input = &b"abcdef"[..];
    assert!(input.sync_skip_exact_unpin(4).is_ok());
    assert_eq!(input, b"ef");
    let mut input = Bytes::from_static(b"abc");
    assert!(input.sync_skip_exact_unpin(3).is_ok());
    assert!(input.is_empty());
}

#[test]
fn eof_reports_what_was_left_to_skip() {
    let mut input = &b"abc"[..];
    let error = input.sync_skip_exact_unpin(5).unwrap_err();
    assert!(matches!(error, SkipExactError::Incomplete { remaining: 2 }));
    assert_eq!(*error.remaining(), 2);
    assert!(error.host_error().is_none());
    assert_eq!(error.to_string(), "source ended with 2 bytes left to skip");
    assert!(input.is_empty());
}

#[test]
fn a_host_failure_reports_what_was_left_and_the_host_error() {
    let mut input = ComposedReceiver::new(ThenFail(b"ab"), ReadSkip);
    let error = input.sync_skip_exact_unpin(5).unwrap_err();
    assert_eq!(*error.remaining(), 3);
    assert_eq!(error.host_error(), Some(&Broken));
    assert!(matches!(
        error,
        SkipExactError::Receiver {
            source: Broken,
            remaining: 3
        }
    ));
}

#[test]
fn a_zero_count_completes_without_polling_the_source() {
    let mut input = ComposedReceiver::new(ThenFail(b""), ReadSkip);
    assert!(input.sync_skip_exact_unpin(0).is_ok());
}

#[test]
fn skips_many_polls_through_scratch_reads() {
    let bytes = [1; 20_000];
    let mut input = ComposedReceiver::new(&bytes[..], ReadSkip);
    assert!(input.sync_skip_exact_unpin(19_999).is_ok());
    assert_eq!(input.into_parts().0, [1]);
}

#[test]
fn skips_to_the_end() {
    let mut input = &b"abcdef"[..];
    assert!(input.sync_skip_to_end_unpin().is_ok());
    assert!(input.is_empty());
    let mut framed = ComposedReceiver::new(&b"abcdef"[..], ReadLimit(4u8));
    assert!(framed.sync_skip_to_end_unpin().is_ok());
    assert_eq!(framed.get_ref(), (&&b"ef"[..], &ReadLimit(0)));
}

#[test]
fn skipping_to_the_end_returns_the_host_error() {
    let mut input = ComposedReceiver::new(ThenFail(b"abc"), ReadSkip);
    assert_eq!(input.sync_skip_to_end_unpin(), Err(Broken));
}

/// Skips `length` bytes within a budget of `budget` on the access, then the
/// rest of the budget, and returns both results.
#[call(sync)]
async fn skip_in_frame<F: Skip<Count = usize>>(
    io: &mut impl ReceiverAccess<Family = F>,
    budget: u16,
    length: usize,
) -> (
    Result<(), SkipExactError<F::Error, usize>>,
    Result<(), F::Error>,
) {
    let mut frame = io.compose(ReadLimit(budget));
    let exact = frame.skip_exact(length).await;
    let rest = frame.skip_to_end().await;
    (exact, rest)
}

#[test]
fn both_operations_run_inside_an_operation() {
    let mut input = &b"abcdefgh"[..];
    let (exact, rest) = input.sync_skip_in_frame_unpin(5, 2);
    assert!(exact.is_ok() && rest.is_ok());
    assert_eq!(input, b"fgh");
    let (exact, rest) = input.sync_skip_in_frame_unpin(2, 3);
    assert_eq!(*exact.unwrap_err().remaining(), 1);
    assert!(rest.is_ok());
    assert_eq!(input, b"h");
}

#[test]
fn skipping_to_the_end_offers_an_infinite_maximum() {
    let mut input = ComposedReceiver::new(&b"abc"[..], ReadLimit(Infinite));
    assert!(input.sync_skip_to_end_unpin().is_ok());
    assert!(input.get_ref().0.is_empty());
}

/// Reads its bytes, then fails.
struct ThenFail(&'static [u8]);
#[derive(Debug, PartialEq)]
struct Broken;
nitori_call::family_receiver!(impl [] for ThenFail);
impl Read for ThenFail {
    type Error = Broken;
    type Metadata = ();
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<Option<nitori_io::ReadSegment<Self::Metadata>>, Broken>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if this.0.is_empty() {
            return Poll::Ready(Err(Broken));
        }
        let count = Ord::min(out.remaining_mut(), this.0.len());
        out.put_slice(&this.0[..count]);
        this.0 = &this.0[count..];
        Poll::Ready(Ok(segment(count)))
    }
}

fn segment(count: usize) -> Option<nitori_io::ReadSegment<()>> {
    core::num::NonZeroUsize::new(count).map(|count| nitori_io::ReadSegment {
        count,
        metadata: (),
    })
}

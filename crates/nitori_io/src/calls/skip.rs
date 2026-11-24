//! Skip operations and their operation-specific failures.
use crate::{Infinite, Min, Skip as SkipHost, SkipExt as _};
use core::ops::SubAssign;
use nitori_call::{ReceiverAccess, call};
use snafu::{IntoError, Snafu};

/// Skip exactly `length` bytes, reporting premature EOF with what was left.
///
/// Each poll asks the source for at most the remaining count, so the source
/// never skips past `length`. A zero length completes without polling the
/// source.
///
/// The count needs `Clone`, `Min<Output = Count>` to offer the remainder as
/// the maximum, `Min<usize, Output = usize>` to tell whether the remainder is
/// zero (its minimum with one), and `SubAssign` to deduct each skip. No
/// comparison beyond `Min` is needed: a source skips at most the maximum, so
/// the deduction cannot underflow.
#[call(sync)]
pub async fn skip_exact<F>(
    io: &mut impl ReceiverAccess<Family = F>,
    length: <F as SkipHost>::Count,
) -> Result<(), SkipExactError<F::Error, <F as SkipHost>::Count>>
where
    F: SkipHost,
    <F as SkipHost>::Count:
        Clone + Min<Output = <F as SkipHost>::Count> + Min<usize, Output = usize> + SubAssign,
{
    let mut remaining = length;
    while Min::<usize>::min(remaining.clone(), 1) != 0 {
        match io.skip(remaining.clone()).await {
            Ok(Some(count)) => remaining -= count,
            Ok(None) => return Err(skip_exact_error::IncompleteSnafu { remaining }.build()),
            Err(source) => {
                return Err(skip_exact_error::ReceiverSnafu { remaining }.into_error(source));
            }
        }
    }
    Ok(())
}

/// Failures that [`skip_exact`] can produce. Each carries the count that was
/// still to be skipped, which is never zero.
#[derive(Debug, Snafu)]
#[snafu(module(skip_exact_error), visibility(pub(crate)))]
pub enum SkipExactError<E, C> {
    /// The source reached EOF before the requested count was skipped.
    #[snafu(display("source ended with {remaining} bytes left to skip"))]
    Incomplete { remaining: C },
    /// The host failed while skipping; `remaining` is the count still to be
    /// skipped when it did.
    #[snafu(display("skip failed with {remaining} bytes left"))]
    Receiver { source: E, remaining: C },
}

impl<E, C> SkipExactError<E, C> {
    /// The count that was still to be skipped when the operation failed.
    pub fn remaining(&self) -> &C {
        match self {
            Self::Incomplete { remaining } | Self::Receiver { remaining, .. } => remaining,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::Incomplete { .. } => None,
        }
    }
}

/// Skip until EOF. The total is not reported, since it need not fit the
/// source's `Count`; query [`ExactLen`](crate::ExactLen) first to know it.
/// The only failure is the source's own, which is returned as it is.
#[call(sync)]
pub async fn skip_to_end<F>(io: &mut impl ReceiverAccess<Family = F>) -> Result<(), F::Error>
where
    F: SkipHost,
    Infinite: Min<<F as SkipHost>::Count, Output = <F as SkipHost>::Count>,
{
    while io.skip(Infinite).await?.is_some() {}
    Ok(())
}

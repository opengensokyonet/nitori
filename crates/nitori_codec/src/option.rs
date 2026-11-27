//! A value that may be absent at the end of its source.
use crate::{DecodeOn, scoped::decode_scoped, staged::AttachIo};
use nitori_call::ReceiverAccess;
use nitori_io::{AtEnd, AtEndExt as _};
use snafu::{ResultExt as _, Snafu};

/// Decodes `None` where the source has ended, and otherwise a `T`, in
/// whose decoding the end of the source is an error of `T`'s.
///
/// The source answers whether it has ended without giving up a byte
/// ([`AtEnd`]). The `T` is a value of its own inside the `Option`: its
/// start and end are reported to the source as for any field. A `T` that
/// keeps the source keeps it in `Some`; at the end, a source given by value
/// is dropped with the `None`.
impl<S, T> DecodeOn<S> for Option<T>
where
    S: ReceiverAccess<Family: AtEnd>,
    T: DecodeOn<S>,
    Option<T::Decoded>: AttachIo<S, Option<T>>,
{
    type Error = DecodeOptionError<<S::Family as AtEnd>::RemainderError, T::Error>;
    type Decoded = Option<T::Decoded>;
    async fn decode_on(io: &mut S) -> Result<Option<T::Decoded>, Self::Error> {
        if io.at_end().await.context(decode_option_error::AtEndSnafu)? {
            return Ok(None);
        }
        decode_scoped::<S, T>(io)
            .await
            .map(Some)
            .context(decode_option_error::ValueSnafu)
    }
}

/// Why a value that may be absent failed to decode, from a source whose
/// remainder queries fail with `E`, into a value that fails with `V`.
#[derive(Debug, Snafu)]
#[snafu(module(decode_option_error), visibility(pub(crate)))]
pub enum DecodeOptionError<E, V> {
    /// The source could not tell whether it had ended.
    #[snafu(display("the source could not tell whether it had ended"))]
    AtEnd { source: E },
    /// The source had not ended, and the value failed.
    #[snafu(display("the value failed"))]
    Value { source: V },
}

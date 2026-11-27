//! The value scope the framework draws around each decoding and encoding.
use crate::{DecodeFrom, DecodeOn, EncodeInto, EncodeOn, staged::AttachIo};
use nitori_call::{
    __private::{ValueEvent, report_value},
    ReceiverAccess,
};

/// Only the framework implements [`DecodeFrom`] and [`EncodeInto`].
#[derive(Debug)]
pub struct Framework(());

/// Reports the end of a value: abandoned unless it was finished.
struct Scope<'a, S: ReceiverAccess> {
    io: &'a mut S,
    finished: Option<ValueEvent>,
    abandoned: ValueEvent,
}

impl<S: ReceiverAccess> Drop for Scope<'_, S> {
    fn drop(&mut self) {
        report_value(self.io, self.finished.take().unwrap_or(self.abandoned));
    }
}

/// Decode a `V` on `io` inside its value scope.
pub(crate) async fn decode_scoped<S: ReceiverAccess, V: DecodeOn<S>>(
    io: &mut S,
) -> Result<V::Decoded, V::Error> {
    report_value(io, ValueEvent::DecodeStarted);
    let mut scope = Scope {
        io,
        finished: None,
        abandoned: ValueEvent::DecodeAbandoned,
    };
    let decoded = V::decode_on(&mut *scope.io).await?;
    scope.finished = Some(ValueEvent::DecodeFinished);
    Ok(decoded)
}

/// Encode `value` on `io` inside its value scope.
pub(crate) async fn encode_scoped<S: ReceiverAccess, V: EncodeOn<S>>(
    value: V,
    io: &mut S,
) -> Result<V::Encoded, V::Error> {
    report_value(io, ValueEvent::EncodeStarted);
    let mut scope = Scope {
        io,
        finished: None,
        abandoned: ValueEvent::EncodeAbandoned,
    };
    let encoded = value.encode_on(&mut *scope.io).await?;
    scope.finished = Some(ValueEvent::EncodeFinished);
    Ok(encoded)
}

/// A value decoded on the access it is given: the start and the end of the
/// value are reported to the access, and a value that keeps the source gets
/// it once it is finished.
impl<S: ReceiverAccess, V: DecodeOn<S>> DecodeFrom<S> for V {
    type Error = V::Error;
    async fn decode_from(mut source: S) -> Result<V, V::Error> {
        let decoded = decode_scoped::<S, V>(&mut source).await?;
        Ok(decoded.attach_io(source))
    }
    fn __framework(_: Framework) {}
}

/// A value encoded on the access it is given, as [`DecodeFrom`] decodes.
impl<S: ReceiverAccess, V: EncodeOn<S>> EncodeInto<S> for V {
    type Output = V::Output;
    type Error = V::Error;
    async fn encode_into(self, mut sink: S) -> Result<V::Output, V::Error> {
        let encoded = encode_scoped(self, &mut sink).await?;
        Ok(encoded.attach_io(sink))
    }
    fn __framework(_: Framework) {}
}

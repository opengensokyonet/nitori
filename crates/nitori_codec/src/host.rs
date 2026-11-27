//! Encoding and decoding as the whole operation of a call on an executor.
//!
//! The value is decoded from, or encoded into, the slot the call lends its
//! operation, so it cannot keep the slot: a value decoded or encoded in
//! stages is used inside an operation through [`Decode`](crate::Decode) and
//! [`Encode`](crate::Encode), or on a receiver held by value through
//! [`DecodeFrom`] or [`EncodeInto`] with [`OnReceiver`](nitori_call::OnReceiver).
use crate::{DecodeFrom, EncodeInto};
use core::{
    future::Future,
    marker::PhantomData,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use nitori_call::{
    CallExecutor, Emitter, Operation, ReceiverFamily, ReceiverSlot, Receivers, RootOperation, Start,
};

/// The operation that decodes a `V` from the receiver of a call: what
/// [`DecodeHostExt`] runs.
#[must_use = "operations do nothing until started and polled"]
pub struct DecodeOperation<V> {
    value: PhantomData<fn() -> V>,
}
impl<V> DecodeOperation<V> {
    /// Decode a `V`.
    pub fn new() -> Self {
        Self { value: PhantomData }
    }
}
impl<V> Default for DecodeOperation<V> {
    fn default() -> Self {
        Self::new()
    }
}
impl<F: ReceiverFamily, V, E> Operation<F> for DecodeOperation<V>
where
    V: for<'s> DecodeFrom<ReceiverSlot<'s, F>, Error = E>,
{
    type Output = Result<V, E>;
    type Yield = !;
}
impl<'s, F: ReceiverFamily, V, E> Start<'s, ReceiverSlot<'s, F>> for DecodeOperation<V>
where
    V: for<'t> DecodeFrom<ReceiverSlot<'t, F>, Error = E>,
{
    type Fut = impl Future<Output = Result<V, E>>;
    fn start(self, slot: ReceiverSlot<'s, F>, _: Emitter<'s, !>) -> Self::Fut {
        V::decode_from(slot)
    }
}

/// The operation that encodes a `V` into the receiver of a call: what
/// [`EncodeHostExt`] runs.
#[must_use = "operations do nothing until started and polled"]
pub struct EncodeOperation<V> {
    value: V,
}
impl<V> EncodeOperation<V> {
    /// Encode `value`.
    pub fn new(value: V) -> Self {
        Self { value }
    }
}
impl<F: ReceiverFamily, V, O, E> Operation<F> for EncodeOperation<V>
where
    V: for<'s> EncodeInto<ReceiverSlot<'s, F>, Output = O, Error = E>,
{
    type Output = Result<O, E>;
    type Yield = !;
}
impl<'s, F: ReceiverFamily, V, O, E> Start<'s, ReceiverSlot<'s, F>> for EncodeOperation<V>
where
    V: for<'t> EncodeInto<ReceiverSlot<'t, F>, Output = O, Error = E>,
{
    type Fut = impl Future<Output = Result<O, E>>;
    fn start(self, slot: ReceiverSlot<'s, F>, _: Emitter<'s, !>) -> Self::Fut {
        self.value.encode_into(slot)
    }
}

/// Decode values as the whole operation of a call on this executor:
/// `input.sync_decode_unpin::<V>()` on a memory buffer.
///
/// Implement [`DecodeOn`](crate::DecodeOn) for accesses of the executor's
/// family to use a value here. The framework runs it through [`DecodeFrom`]
/// on the call slot.
/// The `sync_` versions are for executors that never wait, such as memory
/// buffers, and panic if the decoding returns `Pending`.
pub trait DecodeHostExt: CallExecutor {
    /// Decode a `V` from this executor.
    fn decode<'d, V>(self: Pin<&'d mut Self>) -> Self::Execution<'d, DecodeOperation<V>>
    where
        Self::Receivers: Receivers,
        DecodeOperation<V>: RootOperation<Self::Receivers> + 'd,
    {
        self.execute(DecodeOperation::new())
    }
    /// [`decode`](Self::decode) on an executor that is `Unpin`.
    fn decode_unpin<'d, V>(&'d mut self) -> Self::Execution<'d, DecodeOperation<V>>
    where
        Self: Unpin,
        Self::Receivers: Receivers,
        DecodeOperation<V>: RootOperation<Self::Receivers> + 'd,
    {
        Self::decode(Pin::new(self))
    }
    /// Decode a `V` from an executor that never waits.
    fn sync_decode<'d, V>(
        self: Pin<&'d mut Self>,
    ) -> <DecodeOperation<V> as Operation<Self::Receivers>>::Output
    where
        Self::Receivers: Receivers,
        DecodeOperation<V>: RootOperation<Self::Receivers> + 'd,
    {
        complete_now(Self::decode(self))
    }
    /// [`sync_decode`](Self::sync_decode) on an executor that is `Unpin`.
    fn sync_decode_unpin<'d, V>(
        &'d mut self,
    ) -> <DecodeOperation<V> as Operation<Self::Receivers>>::Output
    where
        Self: Unpin,
        Self::Receivers: Receivers,
        DecodeOperation<V>: RootOperation<Self::Receivers> + 'd,
    {
        Self::sync_decode(Pin::new(self))
    }
}
impl<H: CallExecutor + ?Sized> DecodeHostExt for H {}

/// Encode values as the whole operation of a call on this executor:
/// `output.sync_encode_unpin(value)` on a memory buffer.
///
/// Implement [`EncodeOn`](crate::EncodeOn) for accesses of the executor's
/// family to use a value here. The framework runs it through [`EncodeInto`]
/// on the call slot.
/// The `sync_` versions are for executors that never wait and panic if the
/// encoding returns `Pending`.
pub trait EncodeHostExt: CallExecutor {
    /// Encode `value` into this executor.
    fn encode<'d, V>(self: Pin<&'d mut Self>, value: V) -> Self::Execution<'d, EncodeOperation<V>>
    where
        Self::Receivers: Receivers,
        EncodeOperation<V>: RootOperation<Self::Receivers> + 'd,
    {
        self.execute(EncodeOperation::new(value))
    }
    /// [`encode`](Self::encode) on an executor that is `Unpin`.
    fn encode_unpin<'d, V>(&'d mut self, value: V) -> Self::Execution<'d, EncodeOperation<V>>
    where
        Self: Unpin,
        Self::Receivers: Receivers,
        EncodeOperation<V>: RootOperation<Self::Receivers> + 'd,
    {
        Self::encode(Pin::new(self), value)
    }
    /// Encode `value` into an executor that never waits.
    fn sync_encode<'d, V>(
        self: Pin<&'d mut Self>,
        value: V,
    ) -> <EncodeOperation<V> as Operation<Self::Receivers>>::Output
    where
        Self::Receivers: Receivers,
        EncodeOperation<V>: RootOperation<Self::Receivers> + 'd,
    {
        complete_now(Self::encode(self, value))
    }
    /// [`sync_encode`](Self::sync_encode) on an executor that is `Unpin`.
    fn sync_encode_unpin<'d, V>(
        &'d mut self,
        value: V,
    ) -> <EncodeOperation<V> as Operation<Self::Receivers>>::Output
    where
        Self: Unpin,
        Self::Receivers: Receivers,
        EncodeOperation<V>: RootOperation<Self::Receivers> + 'd,
    {
        Self::sync_encode(Pin::new(self), value)
    }
}
impl<H: CallExecutor + ?Sized> EncodeHostExt for H {}

/// Complete `future` in one poll, for the `sync_` versions.
///
/// # Panics
///
/// If the future returns `Pending`: the executor waited.
fn complete_now<T>(future: impl Future<Output = T>) -> T {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("a synchronous encoding or decoding returned Pending"),
    }
}

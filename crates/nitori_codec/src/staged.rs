//! Values decoded or encoded in stages on a receiver held by value.
use crate::{
    DecodeFrom, EncodeInto,
    host::{DecodeHostExt, EncodeHostExt},
};
use core::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::{OnReceiver, Receiver, ReceiverFamily, ReceiverSlot};

/// Derive [`MapIo`] for a struct or enum that holds IO.
pub use nitori_call_macros::MapIo;

/// A value that holds an IO object that can be replaced.
pub trait MapIo: Sized {
    /// The IO object held by this value.
    type Io;
    /// The same value keeping a `U` instead.
    type With<U>: MapIo<Io = U>;
    /// Replace the IO object, keeping the rest.
    ///
    /// Implementations must call `f` exactly once with the held IO and keep
    /// its result in the returned value. The [`MapIo`] derive follows this
    /// contract, delegating to `MapIo::map_io` for a nested IO field.
    fn map_io<U>(self, f: impl FnOnce(Self::Io) -> U) -> Self::With<U>;
}

/// The slot left by [`take_io`] or by a call returning a value that holds IO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Vacant(());

/// The parts returned by [`take_io`].
#[derive(Debug)]
pub struct IoParts<V, IO> {
    /// The value with a [`Vacant`] slot in place of its IO.
    pub value: V,
    /// The extracted IO.
    pub io: IO,
}

/// `V` with its source taken out.
struct Detached<V: MapIo>(V::With<Vacant>);

pin_project_lite::pin_project! {
    /// Map a future's successful output once and pass its error through.
    ///
    /// This stores the input future directly, including one returned through
    /// a generic associated type.
    ///
    /// # Panics
    ///
    /// Polling after the future returns [`Poll::Ready`] may panic.
    pub struct MapOk<Fut, M> { #[pin] fut: Fut, map: Option<M> }
}
impl<Fut, M> MapOk<Fut, M> {
    /// Map the success of `fut` with `map`.
    pub fn new(fut: Fut, map: M) -> Self {
        Self {
            fut,
            map: Some(map),
        }
    }
}
impl<Fut: Future<Output = Result<T, E>>, T, U, E, M: FnOnce(T) -> U> Future for MapOk<Fut, M> {
    type Output = Result<U, E>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<U, E>> {
        let this = self.project();
        let value = ready!(this.fut.poll(cx))?;
        Poll::Ready(Ok((this.map.take().expect("polled after completion"))(
            value,
        )))
    }
}

fn vacate<T: MapIo>(value: T) -> T::With<Vacant> {
    value.map_io(|_| Vacant(()))
}

impl<'a, F, V, E> DecodeFrom<ReceiverSlot<'a, F>> for Detached<V>
where
    F: ReceiverFamily,
    V: MapIo,
    V::With<ReceiverSlot<'a, F>>:
        DecodeFrom<ReceiverSlot<'a, F>, Error = E> + MapIo<With<Vacant> = V::With<Vacant>>,
{
    type Error = E;
    fn decode_from(slot: ReceiverSlot<'a, F>) -> impl Future<Output = Result<Self, E>> {
        MapOk::new(
            <V::With<ReceiverSlot<'a, F>> as DecodeFrom<_>>::decode_from(slot),
            |value: V::With<ReceiverSlot<'a, F>>| Detached::<V>(vacate(value)),
        )
    }
}

type Slot<'a, R> = ReceiverSlot<'a, <R as Receiver>::Family>;

/// A value that keeps the receiver it is decoded from, decoded by a call on
/// the receiver held by value.
impl<R, V, E> DecodeFrom<R, OnReceiver> for V
where
    R: Receiver + Unpin,
    V: MapIo<Io = R>,
    V::With<Vacant>: MapIo<With<R> = V>,
    for<'a> V::With<Slot<'a, R>>:
        DecodeFrom<Slot<'a, R>, Error = E> + MapIo<With<Vacant> = V::With<Vacant>>,
{
    type Error = E;
    async fn decode_from(mut receiver: R) -> Result<V, E> {
        let Detached(value) = receiver.decode_unpin::<Detached<V>>().await?;
        Ok(value.map_io(|_| receiver))
    }
}

/// A value written in stages whose output keeps the sink, with the sink
/// taken out of it.
struct Detaching<V, D>(V, PhantomData<fn() -> D>);

impl<'a, F, V, D, E> EncodeInto<ReceiverSlot<'a, F>> for Detaching<V, D>
where
    F: ReceiverFamily,
    V: EncodeInto<ReceiverSlot<'a, F>, Error = E>,
    V::Output: MapIo<With<Vacant> = D>,
{
    type Output = D;
    type Error = E;
    fn encode_into(self, slot: ReceiverSlot<'a, F>) -> impl Future<Output = Result<D, E>> {
        MapOk::new(self.0.encode_into(slot), vacate::<V::Output>)
    }
}

/// A value whose output keeps the sink it is encoded into, encoded by a
/// call on the receiver held by value.
impl<R, V, D, O, E> EncodeInto<R, OnReceiver> for V
where
    R: Receiver + Unpin,
    for<'a> V: EncodeInto<Slot<'a, R>, Error = E>,
    for<'a> <V as EncodeInto<Slot<'a, R>>>::Output: MapIo<With<Vacant> = D>,
    D: MapIo<With<R> = O>,
{
    type Output = O;
    type Error = E;
    async fn encode_into(self, mut receiver: R) -> Result<O, E> {
        let output = receiver
            .encode_unpin(Detaching::<V, D>(self, PhantomData))
            .await?;
        Ok(output.map_io(|_| receiver))
    }
}

/// Extract the IO and leave a [`Vacant`] slot in the value.
/// Use [`MapIo::map_io`] to fill the slot again.
///
/// # Panics
///
/// Panics if the [`MapIo::map_io`] implementation returns without calling
/// its mapping function.
pub fn take_io<T: MapIo>(value: T) -> IoParts<T::With<Vacant>, T::Io> {
    let mut io = None;
    let value = value.map_io(|extracted| {
        io = Some(extracted);
        Vacant(())
    });
    IoParts {
        value,
        io: io.expect("MapIo::map_io must call its mapping function"),
    }
}

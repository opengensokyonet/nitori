//! Values decoded or encoded in stages on a receiver held by value.
use crate::{
    DecodeFrom, DecodeOn, EncodeInto, EncodeOn,
    host::{DecodeHostExt, EncodeHostExt},
    scoped::Framework,
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

/// A value that keeps its IO, decoded or encoded with the IO's place
/// vacant: what [`DecodeOn`] or [`EncodeOn`] gives for such a value, which
/// the framework completes with the IO once the value is finished.
///
/// ```
/// # #![feature(impl_trait_in_assoc_type)]
/// use nitori_call::ReceiverAccess;
/// use nitori_codec::{prelude::*, DecodeOn, staged::{KeepsIo, MapIo}};
/// use nitori_io::{Read, calls::ReadArrayError};
///
/// /// A tag, and the source the rest is read from.
/// #[derive(MapIo)]
/// #[map_io(I)]
/// struct Tagged<I> {
///     tag: u8,
///     io: I,
/// }
///
/// impl<A: ReceiverAccess<Family: Read<Metadata = ()>>> DecodeOn<A> for Tagged<A> {
///     type Error = ReadArrayError<<A::Family as Read>::Error>;
///     type Decoded = KeepsIo<Self>;
///     async fn decode_on(io: &mut A) -> Result<KeepsIo<Self>, Self::Error> {
///         let [tag] = io.decode::<[u8; 1]>().await?;
///         Ok(KeepsIo::new(|io| Tagged { tag, io }))
///     }
/// }
/// ```
pub struct KeepsIo<V: MapIo>(V::With<Vacant>);

impl<V: MapIo> KeepsIo<V> {
    /// The value `build` makes around the vacant place of its IO.
    pub fn new(build: impl FnOnce(Vacant) -> V::With<Vacant>) -> Self {
        Self(build(Vacant(())))
    }
}

impl<V: MapIo<With<Vacant>: core::fmt::Debug>> core::fmt::Debug for KeepsIo<V> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("KeepsIo").field(&self.0).finish()
    }
}

mod attach_sealed {
    pub trait Sealed<S, V> {}
}

/// What [`DecodeOn`] or [`EncodeOn`] gave, made into the value or output
/// with the IO it was decoded from or encoded into: a value that keeps
/// nothing drops the borrowed IO's owner, [`KeepsIo`] puts the IO in, and
/// an `Option` of it does so when there is a value. Only this crate
/// implements it.
pub trait AttachIo<S, V>: attach_sealed::Sealed<S, V> {
    /// Make the value with `io`.
    fn attach_io(self, io: S) -> V;
}

impl<S, V> attach_sealed::Sealed<S, V> for V {}
/// A value that keeps nothing of the IO.
impl<S, V> AttachIo<S, V> for V {
    fn attach_io(self, _: S) -> V {
        self
    }
}

impl<S, V> attach_sealed::Sealed<S, V> for KeepsIo<V>
where
    V: MapIo<Io = S>,
    V::With<Vacant>: MapIo<With<S> = V>,
{
}
/// A value that keeps the IO: the IO takes its vacant place.
impl<S, V> AttachIo<S, V> for KeepsIo<V>
where
    V: MapIo<Io = S>,
    V::With<Vacant>: MapIo<With<S> = V>,
{
    fn attach_io(self, io: S) -> V {
        self.0.map_io(|_| io)
    }
}

impl<S, V> attach_sealed::Sealed<S, Option<V>> for Option<KeepsIo<V>>
where
    V: MapIo<Io = S>,
    V::With<Vacant>: MapIo<With<S> = V>,
{
}
/// A value that may be absent and keeps the IO when present: the IO takes
/// its vacant place, or is dropped with `None`.
impl<S, V> AttachIo<S, Option<V>> for Option<KeepsIo<V>>
where
    V: MapIo<Io = S>,
    V::With<Vacant>: MapIo<With<S> = V>,
{
    fn attach_io(self, io: S) -> Option<V> {
        self.map(|kept| kept.attach_io(io))
    }
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

/// A value that keeps the slot it is decoded on, with the slot taken out.
impl<'a, F, V, E> DecodeOn<ReceiverSlot<'a, F>> for Detached<V>
where
    F: ReceiverFamily,
    V: MapIo,
    V::With<ReceiverSlot<'a, F>>:
        DecodeOn<ReceiverSlot<'a, F>, Error = E> + MapIo<With<Vacant> = V::With<Vacant>>,
    <V::With<ReceiverSlot<'a, F>> as DecodeOn<ReceiverSlot<'a, F>>>::Decoded:
        Into<KeepsIo<V::With<ReceiverSlot<'a, F>>>>,
{
    type Error = E;
    fn decode_on(slot: &mut ReceiverSlot<'a, F>) -> impl Future<Output = Result<Self, E>> {
        MapOk::new(
            <V::With<ReceiverSlot<'a, F>> as DecodeOn<_>>::decode_on(slot),
            |decoded: <V::With<ReceiverSlot<'a, F>> as DecodeOn<_>>::Decoded| {
                let KeepsIo(value) = decoded.into();
                Detached::<V>(value)
            },
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
        DecodeOn<Slot<'a, R>, Error = E> + MapIo<With<Vacant> = V::With<Vacant>>,
    for<'a> <V::With<Slot<'a, R>> as DecodeOn<Slot<'a, R>>>::Decoded:
        Into<KeepsIo<V::With<Slot<'a, R>>>>,
{
    type Error = E;
    async fn decode_from(mut receiver: R) -> Result<V, E> {
        let Detached(value) = receiver.decode_unpin::<Detached<V>>().await?;
        Ok(value.map_io(|_| receiver))
    }
    fn __framework(_: Framework) {}
}

/// A value written in stages whose output keeps the sink, with the sink
/// taken out of it.
struct Detaching<V, D>(V, PhantomData<fn() -> D>);

impl<'a, F, V, D, E> EncodeOn<ReceiverSlot<'a, F>> for Detaching<V, D>
where
    F: ReceiverFamily,
    V: EncodeOn<ReceiverSlot<'a, F>, Error = E>,
    V::Output: MapIo<With<Vacant> = D>,
    V::Encoded: Into<KeepsIo<V::Output>>,
{
    type Output = D;
    type Error = E;
    fn encode_on(self, slot: &mut ReceiverSlot<'a, F>) -> impl Future<Output = Result<D, E>> {
        MapOk::new(self.0.encode_on(slot), |encoded: V::Encoded| {
            let KeepsIo(output) = encoded.into();
            output
        })
    }
}

/// A value whose output keeps the sink it is encoded into, encoded by a
/// call on the receiver held by value.
impl<R, V, D, O, E> EncodeInto<R, OnReceiver> for V
where
    R: Receiver + Unpin,
    for<'a> V: EncodeOn<Slot<'a, R>, Error = E>,
    for<'a> <V as EncodeOn<Slot<'a, R>>>::Output: MapIo<With<Vacant> = D>,
    for<'a> <V as EncodeOn<Slot<'a, R>>>::Encoded:
        Into<KeepsIo<<V as EncodeOn<Slot<'a, R>>>::Output>>,
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
    fn __framework(_: Framework) {}
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

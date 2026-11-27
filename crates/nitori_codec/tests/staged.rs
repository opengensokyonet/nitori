//! Staged decoders and encoders with the same entry points on accesses and receivers.
#![feature(impl_trait_in_assoc_type)]
use core::marker::PhantomData;
use nitori_call::{AccessMode, AccessOperation, Receiver, ReceiverAccess, call};
use nitori_codec::{
    DecodeOn, EncodeOn,
    prelude::*,
    staged::{IoParts, KeepsIo, MapIo, MapOk, Vacant, take_io},
};
use nitori_io::{
    Read, Write,
    adapters::ReadLimit,
    calls::{ReadArrayError, WriteArrayError},
};

#[derive(MapIo)]
#[map_io(I)]
struct Held<I> {
    tag: u8,
    io: I,
}

impl<A> DecodeOn<A> for Held<A>
where
    A: ReceiverAccess<Family: Read<Metadata = ()>>,
{
    type Error = ReadArrayError<<A::Family as Read>::Error>;
    type Decoded = KeepsIo<Self>;

    async fn decode_on(io: &mut A) -> Result<KeepsIo<Self>, Self::Error> {
        let [tag] = io.decode::<[u8; 1]>().await?;
        Ok(KeepsIo::new(|io| Held { tag, io }))
    }
}

struct Tag(u8);

impl<A> EncodeOn<A> for Tag
where
    A: ReceiverAccess<Family: Write<(), Report = ()>>,
{
    type Output = Held<A>;
    type Error = WriteArrayError<<A::Family as Write<()>>::Error>;
    type Encoded = KeepsIo<Held<A>>;

    async fn encode_on(self, io: &mut A) -> Result<KeepsIo<Held<A>>, Self::Error> {
        io.encode([self.0]).await?;
        Ok(KeepsIo::new(|io| Held { tag: self.0, io }))
    }
}

fn decode_tag<I, How: AccessMode, E>(io: I) -> impl Future<Output = Result<Held<I>, E>>
where
    Held<I>: DecodeFrom<I, How, Error = E>,
{
    Held::decode_from(io)
}

fn encode_held<I, How: AccessMode, E>(tag: u8, io: I) -> impl Future<Output = Result<Held<I>, E>>
where
    Tag: EncodeInto<I, How, Error = E, Output = Held<I>>,
{
    Tag(tag).encode_into(io)
}

#[call(sync)]
async fn transfer<F, G>(
    input: &mut impl ReceiverAccess<Family = F>,
    output: &mut impl ReceiverAccess<Family = G>,
) -> [u8; 2]
where
    F: Read<Metadata = (), Error = !>,
    G: Write<(), Report = (), Error = !>,
{
    let read = decode_tag(input).await.unwrap();
    let written = encode_held(read.tag, output).await.unwrap();
    let [second] = read.io.decode::<[u8; 1]>().await.unwrap();
    written.io.encode([second]).await.unwrap();
    [written.tag, second]
}

async fn decode_on_receiver<R>(io: R) -> Held<R>
where
    R: Receiver<Family: Read<Metadata = (), Error = !>> + Unpin + Send,
{
    require_send(decode_tag(io)).await.unwrap()
}

async fn encode_on_receiver<R>(io: R) -> Held<R>
where
    R: Receiver<Family: Write<(), Report = (), Error = !>> + Unpin + Send,
{
    require_send(encode_held(b'x', io)).await.unwrap()
}

fn require_send<F: Future + Send>(future: F) -> F {
    future
}

fn send<F: Future + Send>(future: F) -> F::Output {
    nitori_call::__private::run_sync(future)
}

#[test]
fn the_same_entry_points_on_a_slot_and_on_a_receiver() {
    let bytes = *b"xyz";
    let mut input = bytes.as_slice();
    let mut output = Vec::new();
    assert_eq!((&mut input, &mut output).sync_transfer_unpin(), *b"xy");
    assert_eq!(input, b"z");
    assert_eq!(output, b"xy");

    let Held { tag, mut io } = send(decode_on_receiver(bytes.as_slice()));
    assert_eq!(tag, b'x');
    assert_eq!(io.sync_decode_unpin::<[u8; 1]>().unwrap(), *b"y");
    assert_eq!(io, b"z");

    let mut output = Vec::new();
    let IoParts {
        value: held,
        io: output,
    } = take_io(send(encode_on_receiver(&mut output)));
    assert_eq!(held.tag, b'x');
    output.sync_encode_unpin(*b"y").unwrap();
    assert_eq!(output, b"xy");
}

struct DecodeHeld<'b, D>(&'b mut usize, PhantomData<fn() -> D>);

type Bounded<'a, 'b, A> = ReadLimit<&'a mut A, usize, &'b mut usize>;

impl<'b, A, D, E> AccessOperation<A> for DecodeHeld<'b, D>
where
    A: ReceiverAccess<Family: Read<Metadata = ()>>,
    D: MapIo<Io = Vacant>,
    for<'a> D::With<Bounded<'a, 'b, A>>:
        DecodeFrom<Bounded<'a, 'b, A>, Error = E> + MapIo<With<Vacant> = D>,
{
    type Output = Result<(D, &'b mut usize), E>;

    fn run_on(self, io: &mut A) -> impl Future<Output = Self::Output> {
        let future = <D::With<Bounded<'_, 'b, A>> as DecodeFrom<_>>::decode_from(
            ReadLimit::borrowing(io, self.0),
        );
        // Store the projected decode future in MapOk so rustc can prove Send.
        MapOk::new(future, |value: D::With<Bounded<'_, 'b, A>>| {
            let IoParts { value, io: payload } = take_io(value);
            (value, payload.into_parts().quota)
        })
    }
}

#[call]
async fn decode_in_payload<F>(io: &mut impl ReceiverAccess<Family = F>) -> (u8, usize)
where
    F: Read<Metadata = (), Error = !>,
{
    let mut limit = 2;
    let (value, limit) = DecodeHeld::<Held<Vacant>>(&mut limit, PhantomData)
        .run_on(io)
        .await
        .unwrap();
    (value.tag, *limit)
}

async fn decode_in_receiver<R>(io: &mut R, limit: &mut usize) -> (u8, usize)
where
    R: Receiver<Family: Read<Metadata = (), Error = !>> + Unpin + Send,
{
    let operation = DecodeHeld::<Held<Vacant>>(limit, PhantomData);
    let (value, limit) = require_send(operation.run_on(io)).await.unwrap();
    (value.tag, *limit)
}

#[test]
fn a_borrowed_payload_leaves_a_slot_with_its_budget() {
    let bytes = *b"abc";
    let mut receiver = bytes.as_slice();
    assert_eq!(send(receiver.decode_in_payload_unpin()), (b'a', 1));
    let mut limit = 1;
    assert_eq!(
        send(decode_in_receiver(&mut receiver, &mut limit)),
        (b'b', 0)
    );
    assert_eq!(receiver, b"c");
}

//! A value that may be absent at the end of its source.
#![feature(impl_trait_in_assoc_type)]
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{ReceiverAccess, call};
use nitori_codec::{
    DecodeOn,
    option::DecodeOptionError,
    prelude::*,
    staged::{KeepsIo, MapIo},
};
use nitori_io::{AtEnd, Read, calls::ReadArrayError};

#[derive(MapIo)]
#[map_io(I)]
struct Held<I> {
    tag: u8,
    io: I,
}

impl<A: ReceiverAccess<Family: Read<Metadata = ()>>> DecodeOn<A> for Held<A> {
    type Error = ReadArrayError<<A::Family as Read>::Error>;
    type Decoded = KeepsIo<Self>;
    async fn decode_on(io: &mut A) -> Result<KeepsIo<Self>, Self::Error> {
        let [tag] = io.decode::<[u8; 1]>().await?;
        Ok(KeepsIo::new(|io| Held { tag, io }))
    }
}

#[call(sync)]
async fn bytes_until_the_end<F: Read<Metadata = (), Error = !> + AtEnd<RemainderError = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Some([byte]) = io.decode::<Option<[u8; 1]>>().await.unwrap() {
        bytes.push(byte);
    }
    bytes
}

#[call(sync)]
async fn held_until_the_end<F: Read<Metadata = (), Error = !> + AtEnd<RemainderError = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (Vec<(u8, u8)>, bool) {
    let mut pairs = Vec::new();
    loop {
        match Option::<Held<&mut _>>::decode_from(&mut *io).await.unwrap() {
            Some(held) => {
                let [second] = held.io.decode::<[u8; 1]>().await.unwrap();
                pairs.push((held.tag, second));
            }
            None => return (pairs, true),
        }
    }
}

#[test]
fn values_are_present_until_the_source_ends() {
    let mut input = b"abc".as_slice();
    assert_eq!((&mut input).sync_bytes_until_the_end_unpin(), b"abc");
}

#[test]
fn a_value_that_keeps_the_source_keeps_it_in_some() {
    let mut input = b"abcd".as_slice();
    assert_eq!(
        (&mut input).sync_held_until_the_end_unpin(),
        (vec![(b'a', b'b'), (b'c', b'd')], true)
    );
}

#[test]
fn the_end_inside_the_value_is_the_value_s_error() {
    #[call(sync)]
    async fn odd<F: Read<Metadata = (), Error = !> + AtEnd<RemainderError = !>>(
        io: &mut impl ReceiverAccess<Family = F>,
    ) -> bool {
        matches!(
            io.decode::<Option<[u8; 2]>>().await,
            Err(DecodeOptionError::Value { .. })
        )
    }
    let mut input = b"a".as_slice();
    assert!((&mut input).sync_odd_unpin());
}

#[derive(Debug)]
struct ResetError;

struct Reset;

nitori_call::family_receiver!(impl [] for Reset);

impl AtEnd for Reset {
    type RemainderError = ResetError;

    fn poll_at_end<'v>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
    ) -> Poll<Result<bool, ResetError>>
    where
        Self: 'v,
    {
        Poll::Ready(Err(ResetError))
    }
}

struct Marker;

impl<A: ReceiverAccess> DecodeOn<A> for Marker {
    type Error = !;

    async fn decode_on(_: &mut A) -> Result<Self, !> {
        panic!("the decoder must not be called when the end query fails")
    }
}

#[test]
fn a_failed_end_query_is_reported_without_decoding_the_value() {
    assert!(matches!(
        Reset.sync_decode_unpin::<Option<Marker>>(),
        Err(DecodeOptionError::AtEnd { source: ResetError })
    ));
}

//! The value scope: where each decoded or encoded value starts and ends,
//! as the access's tracker sees it.
#![feature(impl_trait_in_assoc_type)]
use bytes::{Buf, BufMut};
use core::{
    cell::RefCell,
    num::NonZeroUsize,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use std::rc::Rc;

use nitori_call::{AccessPlan, ProvideAccess, ReceiverAccess, TrackValues, call, track_values};
use nitori_codec::{
    prelude::*,
    staged::{KeepsIo, MapIo},
};
use nitori_io::{
    AtEnd, Read, ReadSegment, Write, WriteSegment,
    adapters::{ReadLimit, WriteLimit},
    calls::{ReadArrayError, WriteArrayError},
};

#[derive(Clone, Default)]
struct Log(Rc<RefCell<Vec<&'static str>>>);

impl Log {
    fn push(&self, event: &'static str) {
        self.0.borrow_mut().push(event);
    }

    fn take(&self) -> Vec<&'static str> {
        self.0.take()
    }
}
impl TrackValues for Log {
    fn decode_started(&mut self) {
        self.push("r+");
    }
    fn decode_finished(&mut self) {
        self.push("r.");
    }
    fn decode_abandoned(&mut self) {
        self.push("r!");
    }
    fn encode_started(&mut self) {
        self.push("w+");
    }
    fn encode_finished(&mut self) {
        self.push("w.");
    }
    fn encode_abandoned(&mut self) {
        self.push("w!");
    }
}

/// A source that tracks its values.
struct Tracked<A> {
    io: A,
    log: Log,
}
impl<A: ReceiverAccess> ProvideAccess for Tracked<A> {
    type AccessFamily = A::Family;
    type Lentness = A::Lentness;
    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        track_values(self.io.access_plan(), &mut self.log)
    }
}

/// A holder that forwards the plan of the access it keeps.
struct Holder<A> {
    io: A,
}
impl<A: ReceiverAccess> ProvideAccess for Holder<A> {
    type AccessFamily = A::Family;
    type Lentness = A::Lentness;
    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        self.io.access_plan()
    }
}

/// Two one-byte fields.
#[derive(Debug, PartialEq, Eq)]
struct Pair([u8; 1], [u8; 1]);
impl<A: ReceiverAccess<Family: Read<Metadata = ()>>> DecodeOn<A> for Pair {
    type Error = ReadArrayError<<A::Family as Read>::Error>;
    async fn decode_on(io: &mut A) -> Result<Self, Self::Error> {
        let first = io.decode::<[u8; 1]>().await?;
        let second = io.decode::<[u8; 1]>().await?;
        Ok(Pair(first, second))
    }
}
impl<A: ReceiverAccess<Family: Write<(), Report = ()>>> EncodeOn<A> for Pair {
    type Output = ();
    type Error = WriteArrayError<<A::Family as Write<()>>::Error>;
    async fn encode_on(self, io: &mut A) -> Result<(), Self::Error> {
        io.encode(self.0).await?;
        io.encode(self.1).await
    }
}

#[call(sync)]
async fn decode_tracked<F: Read<Metadata = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (bool, Vec<&'static str>) {
    let mut tracked = Tracked {
        io,
        log: Log::default(),
    };
    let decoded = Pair::decode_from(&mut tracked).await.is_ok();
    (decoded, tracked.log.take())
}

#[call(sync)]
async fn decode_held_tracked<F: Read<Metadata = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (bool, Vec<&'static str>) {
    let mut holder = Holder {
        io: Tracked {
            io,
            log: Log::default(),
        },
    };
    let decoded = Pair::decode_from(&mut &mut &mut holder).await.is_ok();
    (decoded, holder.io.log.take())
}

#[call(sync)]
async fn encode_tracked<G: Write<(), Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = G>,
) -> Vec<&'static str> {
    let mut tracked = Tracked {
        io,
        log: Log::default(),
    };
    Pair(*b"x", *b"y").encode_into(&mut tracked).await.unwrap();
    tracked.log.take()
}

#[test]
fn a_value_and_its_fields_report_their_start_and_end() {
    let mut input = b"ab".as_slice();
    assert_eq!(
        (&mut input).sync_decode_tracked_unpin(),
        (true, vec!["r+", "r+", "r.", "r+", "r.", "r."])
    );
}

#[test]
fn a_value_that_fails_between_its_fields_is_abandoned() {
    let mut input = b"a".as_slice();
    assert_eq!(
        (&mut input).sync_decode_tracked_unpin(),
        (false, vec!["r+", "r+", "r.", "r+", "r!", "r!"])
    );
}

#[test]
fn a_holder_that_forwards_the_plan_forwards_the_reports() {
    let mut input = b"ab".as_slice();
    assert_eq!(
        (&mut input).sync_decode_held_tracked_unpin(),
        (true, vec!["r+", "r+", "r.", "r+", "r.", "r."])
    );
}

#[test]
fn an_encoded_value_reports_its_start_and_end() {
    let mut output = Vec::new();
    assert_eq!(
        output.sync_encode_tracked_unpin(),
        vec!["w+", "w+", "w.", "w+", "w.", "w."]
    );
    assert_eq!(output, b"xy");
}

/// A source and sink that each wait once before transferring a byte.
#[derive(Default)]
struct Waiting {
    input: &'static [u8],
    output: Vec<u8>,
    ready: bool,
}

impl Waiting {
    fn can_transfer(&mut self, cx: &Context<'_>) -> bool {
        self.ready = !self.ready;
        if self.ready {
            cx.waker().wake_by_ref();
        }
        !self.ready
    }
}

nitori_call::family_receiver!(impl [] for Waiting);

impl Read for Waiting {
    type Error = !;
    type Metadata = ();

    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<Option<ReadSegment<()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if !out.has_remaining_mut() {
            return Poll::Ready(Ok(None));
        }
        let Some((&byte, rest)) = this.input.split_first() else {
            return Poll::Ready(Ok(None));
        };
        if !this.can_transfer(cx) {
            return Poll::Pending;
        }
        out.put_u8(byte);
        this.input = rest;
        Poll::Ready(Ok(Some(ReadSegment {
            count: NonZeroUsize::MIN,
            metadata: (),
        })))
    }
}

impl Write<()> for Waiting {
    type Error = !;
    type Report = ();

    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        input: &mut B,
        _: &(),
    ) -> Poll<Result<Option<WriteSegment<()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if !input.has_remaining() {
            return Poll::Ready(Ok(None));
        }
        if !this.can_transfer(cx) {
            return Poll::Pending;
        }
        this.output.push(input.get_u8());
        Poll::Ready(Ok(Some(WriteSegment {
            count: NonZeroUsize::MIN,
            report: (),
        })))
    }
}

#[call]
async fn decode_projected<F: Read<Metadata = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
    log: Log,
) -> Result<Pair, ReadArrayError<!>> {
    let tracked = Tracked { io, log };
    Pair::decode_from(ReadLimit::new(tracked, 2usize)).await
}

#[test]
fn dropping_a_decode_between_fields_abandons_both_open_scopes_through_a_projection() {
    let mut source = Waiting {
        input: b"ab",
        ..Waiting::default()
    };
    let log = Log::default();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut future = pin!(source.decode_projected_unpin(log.clone()));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(*log.0.borrow(), ["r+", "r+", "r.", "r+"]);
    }
    assert_eq!(log.take(), ["r+", "r+", "r.", "r+", "r!", "r!"]);
    assert_eq!(source.input, b"b");
}

#[test]
fn dropping_a_decode_before_its_first_byte_still_reports_abandonment() {
    let mut source = Waiting {
        input: b"ab",
        ..Waiting::default()
    };
    let log = Log::default();
    {
        let mut future = pin!(source.decode_projected_unpin(log.clone()));
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert_eq!(*log.0.borrow(), ["r+", "r+"]);
    }
    assert_eq!(log.take(), ["r+", "r+", "r!", "r!"]);
    assert_eq!(source.input, b"ab");
}

#[call]
async fn encode_waiting<F: Write<(), Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
    log: Log,
) -> Result<(), WriteArrayError<!>> {
    (*b"xy").encode_into(Tracked { io, log }).await
}

#[test]
fn dropping_an_encode_after_one_byte_abandons_the_value() {
    let mut sink = Waiting::default();
    let log = Log::default();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut future = pin!(sink.encode_waiting_unpin(log.clone()));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(*log.0.borrow(), ["w+"]);
    }
    assert_eq!(log.take(), ["w+", "w!"]);
    assert_eq!(sink.output, b"x");
}

struct Held<S> {
    tag: u8,
    io: S,
    log: Log,
}

impl<S> MapIo for Held<S> {
    type Io = S;
    type With<U> = Held<U>;

    fn map_io<U>(self, f: impl FnOnce(S) -> U) -> Held<U> {
        let io = f(self.io);
        self.log.push("attached");
        Held {
            io,
            tag: self.tag,
            log: self.log,
        }
    }
}

impl<A: ReceiverAccess<Family: Read<Metadata = ()>>> DecodeOn<Tracked<A>> for Held<Tracked<A>> {
    type Error = ReadArrayError<<A::Family as Read>::Error>;
    type Decoded = KeepsIo<Self>;

    async fn decode_on(io: &mut Tracked<A>) -> Result<KeepsIo<Self>, Self::Error> {
        let [tag] = io.decode::<[u8; 1]>().await?;
        let log = io.log.clone();
        Ok(KeepsIo::new(|io| Held { tag, io, log }))
    }
}

#[call(sync)]
async fn decode_keeping_access<F: Read<Metadata = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Vec<&'static str> {
    let tracked = Tracked {
        io,
        log: Log::default(),
    };
    let mut held = Held::decode_from(tracked).await.unwrap();
    assert_eq!(held.tag, b'a');
    assert_eq!(held.io.decode::<[u8; 1]>().await.unwrap(), *b"b");
    held.log.take()
}

#[test]
fn a_staged_decoder_reports_completion_before_attaching_the_access() {
    let mut source = b"abc".as_slice();
    assert_eq!(
        source.sync_decode_keeping_access_unpin(),
        ["r+", "r+", "r.", "r.", "attached", "r+", "r."]
    );
    assert_eq!(source, b"c");
}

#[call(sync)]
async fn encode_past_a_budget<G: Write<(), Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = G>,
) -> Vec<&'static str> {
    let mut tracked = Tracked {
        io: WriteLimit::new(io, 1),
        log: Log::default(),
    };
    assert!((*b"ab").encode_into(&mut tracked).await.is_err());
    tracked.log.take()
}

#[test]
fn an_encoding_that_fails_is_abandoned() {
    let mut output = Vec::new();
    assert_eq!(output.sync_encode_past_a_budget_unpin(), vec!["w+", "w!"]);
    assert_eq!(output, b"a");
}

#[call(sync)]
async fn options_until_the_end<F: Read<Metadata = (), Error = !> + AtEnd<RemainderError = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Vec<Vec<&'static str>> {
    let mut tracked = Tracked {
        io,
        log: Log::default(),
    };
    let mut logs = Vec::new();
    assert_eq!(
        tracked.decode::<Option<[u8; 1]>>().await.unwrap(),
        Some(*b"x")
    );
    logs.push(tracked.log.take());
    assert!(tracked.decode::<Option<[u8; 2]>>().await.is_err());
    logs.push(tracked.log.take());
    assert_eq!(tracked.decode::<Option<[u8; 1]>>().await.unwrap(), None);
    logs.push(tracked.log.take());
    logs
}

#[test]
fn an_option_is_a_value_around_its_value() {
    let mut input = b"xy".as_slice();
    assert_eq!(
        (&mut input).sync_options_until_the_end_unpin(),
        vec![
            vec!["r+", "r+", "r.", "r."],
            vec!["r+", "r+", "r!", "r!"],
            vec!["r+", "r."],
        ]
    );
}

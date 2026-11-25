//! A contributed capability used by a call generic over the structure,
//! through what a bound of the composition says of the pieces: frames
//! dispatched by the sum of every part's, two parts with frames and one
//! without.
use alloc::format;

use frunk_core::{
    HList,
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil},
    hlist_pat,
};
use nitori_call::{LentAccess, call};

use super::{Q1, Q2, cx, host, noop};
use crate::{
    call::{CallFamily, CallKind, Composition, LoanNeeds, PartsOf, poll_call, start_calls},
    capability::{Contribute, Contributed, DispatchRun, DispatchSum, PieceList},
    frunk_core::hlist,
    id::Numbered,
    lend::tests::{
        fixtures::{
            Core, LinkCore, LinkId, LinkPath, Loss, PathKey, SentRecords, Watch, lend,
            same_on_both, tree_one_as_two, tree_two,
        },
        parts::{log, take_log},
    },
    need::{Mut, Pos, Ref, View},
    structure::NodeKind,
};

/// The family: frames, each dispatched to its part's piece.
#[derive(Numbered)]
pub struct WireFrames;
/// The core's frame: an acknowledgement.
pub struct Ack(pub u32);
/// The loss part's frame: a loss.
pub struct Lost(pub u32);

/// A frame read off the wire, by its type byte and its value.
pub trait Decode: Sized {
    /// The frame, when the type byte is one of its.
    fn decode(raw: (u8, u32)) -> Option<Self>;
}
impl Decode for Ack {
    fn decode((t, v): (u8, u32)) -> Option<Self> {
        (t == 0).then_some(Ack(v))
    }
}
impl Decode for Lost {
    fn decode((t, v): (u8, u32)) -> Option<Self> {
        (t == 1).then_some(Lost(v))
    }
}
impl Decode for CNil {
    fn decode(_: (u8, u32)) -> Option<Self> {
        None
    }
}
impl<H: Decode, T: Decode> Decode for Coproduct<H, T> {
    fn decode(raw: (u8, u32)) -> Option<Self> {
        H::decode(raw)
            .map(Coproduct::Inl)
            .or_else(|| T::decode(raw).map(Coproduct::Inr))
    }
}

/// The frames of one part, defined once, on the wire's side.
pub trait FramePart {
    /// Its frames.
    type Run;
}
impl FramePart for Core {
    type Run = Ack;
}
impl FramePart for Loss {
    type Run = Lost;
}
impl FramePart for Watch {
    type Run = CNil;
}
/// The frames of a list of parts: the sum of every part's.
pub trait FrameSet {
    /// The sum.
    type Sum;
}
impl FrameSet for HNil {
    type Sum = CNil;
}
impl<H: FramePart, T: FrameSet> FrameSet for HCons<H, T> {
    type Sum = Coproduct<H::Run, T::Sum>;
}
/// The frames of the parts of `Q`.
pub type FramesOf<Q> = <PartsOf<Q> as FrameSet>::Sum;

/// The core's piece: the link's core, which counts what is acknowledged.
pub struct AckPiece<'t>(&'t mut LinkCore);
impl DispatchRun<WireFrames, Ack, (), u32> for AckPiece<'_> {
    fn dispatch_run(&mut self, ack: Ack, _: ()) -> u32 {
        self.0.rx += ack.0;
        self.0.rx
    }
}
/// The loss part's piece: the sent records, which count what is lost.
pub struct LostPiece<'t>(&'t mut SentRecords);
impl DispatchRun<WireFrames, Lost, (), u32> for LostPiece<'_> {
    fn dispatch_run(&mut self, lost: Lost, _: ()) -> u32 {
        self.0.acked += lost.0;
        self.0.acked
    }
}
/// The core's contribution, in its own list.
pub struct CoreFrames;
impl Contribute<WireFrames> for CoreFrames {
    type Needs = HList![Mut<LinkCore>];
    type Piece<'t> = AckPiece<'t>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> AckPiece<'t> {
        let hlist_pat![core] = view;
        AckPiece(core)
    }
}
/// The loss part's contribution, in its own list.
pub struct LossFrames;
impl Contribute<WireFrames> for LossFrames {
    type Needs = HList![Mut<SentRecords>];
    type Piece<'t> = LostPiece<'t>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> LostPiece<'t> {
        let hlist_pat![sent] = view;
        LostPiece(sent)
    }
}

/// What a call that dispatches frames asks of the composition.
pub trait DispatchesFrames:
    Composition<Parts: FrameSet<Sum: Decode>>
    + for<'w> PieceList<WireFrames, Pieces<'w>: DispatchSum<WireFrames, FramesOf<Self>, (), u32>>
{
}
impl<Q> DispatchesFrames for Q where
    Q: Composition<Parts: FrameSet<Sum: Decode>>
        + for<'w> PieceList<WireFrames, Pieces<'w>: DispatchSum<WireFrames, FramesOf<Q>, (), u32>>
{
}

/// A receive: every frame read, dispatched by the sum to its part's piece,
/// and what the piece answered noted; a type byte no part has is noted as
/// unknown.
impl<Q: DispatchesFrames, K: NodeKind> CallKind<Q, K> for Receiving {
    type Needs = HList![Contributed<WireFrames, Q>];
    type Op = Self;
    fn start(_: &mut K::Start) -> Self {
        Receiving::new()
    }
}
#[call]
pub async fn receiving<W: CallFamily<C = Receiving, Q: DispatchesFrames>>(
    host: &mut impl LentAccess<Family = W>,
) {
    host.with(|mut view| {
        let hlist_pat![pieces] = view.needs();
        for raw in [(0, 5), (1, 2), (9, 9), (0, 3), (1, 4)] {
            match FramesOf::<W::Q>::decode(raw) {
                Some(frame) => log(format!("{} -> {}", raw.0, pieces.dispatch_sum(frame, ()))),
                None => log(format!("{} unknown", raw.0)),
            }
        }
    });
}

/// What a receive did: the poll, the log, the link's count and the sent
/// records' count.
type Received = (
    Option<bool>,
    alloc::vec::Vec<alloc::string::String>,
    Option<u32>,
    Option<u32>,
);

#[test]
fn dispatched_through_the_bound() {
    let mut root = tree_one_as_two();
    let mut cx1 = cx::<Q1>();
    let at = hlist![PathKey(10), LinkId(1)];
    let hlist_pat![_, _, _, _, receiving, _] = start_calls::<Q1, LinkPath, _>(&mut 5);
    take_log();
    let mut receiving = core::pin::pin!(receiving);
    let polled = poll_call::<Q1, LinkPath, Receiving, _, _>(
        &mut root,
        &at,
        &mut cx1,
        receiving.as_mut(),
        &mut host(0),
        &mut noop(),
    )
    .map(|p| p.is_ready());
    let one: Received = (
        polled,
        take_log(),
        lend!(&mut root, LinkPath, HList![Ref<LinkCore>], at, |hlist_pat![
            c
        ]| c
            .rx),
        lend!(
            &mut root,
            LinkPath,
            HList![Ref<SentRecords>],
            at,
            |hlist_pat![s]| s.acked
        ),
    );
    let mut root = tree_two();
    let mut cx2 = cx::<Q2>();
    let at = hlist![LinkId(1)];
    let hlist_pat![_, _, _, _, receiving, _] = start_calls::<Q2, LinkPath, _>(&mut 5);
    take_log();
    let mut receiving = core::pin::pin!(receiving);
    let polled = poll_call::<Q2, LinkPath, Receiving, _, _>(
        &mut root,
        &at,
        &mut cx2,
        receiving.as_mut(),
        &mut host(0),
        &mut noop(),
    )
    .map(|p| p.is_ready());
    let two: Received = (
        polled,
        take_log(),
        lend!(&mut root, LinkPath, HList![Ref<LinkCore>], at, |hlist_pat![
            c
        ]| c
            .rx),
        lend!(
            &mut root,
            LinkPath,
            HList![Ref<SentRecords>],
            at,
            |hlist_pat![s]| s.acked
        ),
    );
    same_on_both(
        one,
        two,
        (
            Some(true),
            ["0 -> 5", "1 -> 2", "9 unknown", "0 -> 8", "1 -> 6"]
                .map(alloc::string::String::from)
                .to_vec(),
            Some(8),
            Some(6),
        ),
    );
}

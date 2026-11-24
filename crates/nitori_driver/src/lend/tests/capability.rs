//! Capabilities on both structures: a composed one taken at the lend
//! point, at another node, at every member and in a `+`, by code generic
//! over its position; and contributed ones, by the two rules.
use alloc::vec::Vec;

use frunk_core::{
    HList,
    coproduct::{CNil, Coproduct},
    hlist,
    hlist::HNil,
    hlist_pat,
};

use super::fixtures::{lend, *};
use crate::{
    capability::{
        Capability, Contribute, Contributed, DispatchRun, DispatchSum, EachPiece, EachRun,
    },
    lend::{Visit, lend_visit},
    need::{At, Cap, Members, Mut, Pos, Ref, Under, View},
    needs,
};

/// Counting on a link: its core, exclusive, and its planner.
struct LinkCounting;
struct LinkCount<'t> {
    core: &'t mut LinkCore,
    planner: &'t Planner,
}
impl LinkCount<'_> {
    /// One more on the link: the count and where the planner is.
    fn bump(&mut self) -> (u32, u64) {
        self.core.rx += 1;
        (self.core.rx, self.planner.next_pn)
    }
}
impl Capability for LinkCounting {
    type Needs = HList![Mut<LinkCore>, Ref<Planner>];
    type Object<'t, P: Pos> = LinkCount<'t>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> LinkCount<'t> {
        let hlist_pat![core, planner] = view;
        LinkCount { core, planner }
    }
}
/// Recording a packet number in the sent records.
struct Recording;
impl Capability for Recording {
    type Needs = HList![Mut<SentRecords>];
    type Object<'t, P: Pos> = &'t mut SentRecords;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> &'t mut SentRecords {
        view.head
    }
}

type Here = HList![Cap<LinkCounting>, Ref<Keys>, At<SentRecords, Cap<Recording>>];
/// At the link: count once, and record the planner's number.
struct Once;
impl Visit<Here> for Once {
    type Out = ((u32, u64), u32, usize);
    fn visit<'v, P: Pos>(self, view: View<'v, Here, P>) -> Self::Out {
        let hlist_pat![mut count, keys, records] = view;
        let got = count.bump();
        records.sent.push(got.1);
        (got, keys.epoch, records.sent.len())
    }
}

type Every = HList![Under<ConnCore, At<LinkCore, Cap<LinkCounting>>>];
/// At the connection: count once on every link.
struct OnEvery;
impl Visit<Every> for OnEvery {
    type Out = Vec<(u32, u64)>;
    fn visit<'v, P: Pos>(self, view: View<'v, Every, P>) -> Self::Out {
        let hlist_pat![mut links] = view;
        links.iter_mut().map(|(_, c)| c.bump()).collect()
    }
}

type Either = needs![Ref<Keys>, (Cap<LinkCounting> + Mut<LinkCore>)];
/// At the link: count once, then set the count through the other branch.
struct OneThenOther;
impl Visit<Either> for OneThenOther {
    type Out = ((u32, u64), u32);
    fn visit<'v, P: Pos>(self, view: View<'v, Either, P>) -> Self::Out {
        let hlist_pat![keys, mut plus] = view;
        let got = plus.take::<Cap<LinkCounting>, _>().bump();
        let core = plus.take::<Mut<LinkCore>, _>();
        core.rx += keys.epoch;
        (got, core.rx)
    }
}

#[test]
fn composed() {
    let mut root = tree_one_as_two();
    let one = (
        lend_visit::<_, _, LinkPath, _, Here, _, _, _>(
            &mut root,
            &hlist![PathKey(10), LinkId(2)],
            &mut (),
            Once,
        ),
        lend_visit::<_, _, Conn1, _, Every, _, _, _>(&mut root, &hlist![], &mut (), OnEvery),
        lend_visit::<_, _, LinkPath, _, Either, _, _, _>(
            &mut root,
            &hlist![PathKey(10), LinkId(1)],
            &mut (),
            OneThenOther,
        ),
    );
    let mut root = tree_two();
    let two = (
        lend_visit::<_, _, LinkPath, _, Here, _, _, _>(
            &mut root,
            &hlist![LinkId(2)],
            &mut (),
            Once,
        ),
        lend_visit::<_, _, Conn2, _, Every, _, _, _>(&mut root, &hlist![], &mut (), OnEvery),
        lend_visit::<_, _, LinkPath, _, Either, _, _, _>(
            &mut root,
            &hlist![LinkId(1)],
            &mut (),
            OneThenOther,
        ),
    );
    same_on_both(
        one,
        two,
        (
            Some(((1, 200), 3, 1)),
            Some(alloc::vec![(1, 100), (2, 200)]),
            Some(((2, 100), 5)),
        ),
    );
}

/// The family that counts: the core reads its link's count, the loss part
/// the sent records, the watch gives nothing.
struct Counts;
struct LinkPiece<'t>(&'t LinkCore);
impl EachRun<Counts, u32> for LinkPiece<'_> {
    fn each_run(&mut self, n: &mut u32) {
        *n += self.0.rx + 1;
    }
}
struct RecordsPiece<'t>(&'t SentRecords);
impl EachRun<Counts, u32> for RecordsPiece<'_> {
    fn each_run(&mut self, n: &mut u32) {
        *n = *n * 10 + u32::try_from(self.0.sent.len()).unwrap_or(u32::MAX);
    }
}
impl Contribute<Counts> for Core {
    type Needs = HList![Ref<LinkCore>];
    type Piece<'t> = LinkPiece<'t>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> LinkPiece<'t> {
        LinkPiece(view.head)
    }
}
impl Contribute<Counts> for Loss {
    type Needs = HList![Ref<SentRecords>];
    type Piece<'t> = RecordsPiece<'t>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> RecordsPiece<'t> {
        RecordsPiece(view.head)
    }
}
impl Contribute<Counts> for Watch {
    type Needs = HNil;
    type Piece<'t> = ();
    fn assemble<'t, P: Pos>(_: View<'t, Self::Needs, P>) -> Self::Piece<'t> {}
}

/// The family that takes frames: one alternative per part, the watch's
/// empty.
struct Frames;
struct CoreFrame(u32);
struct LossFrame(u64);
type FrameSum = Coproduct<CoreFrame, Coproduct<LossFrame, Coproduct<CNil, CNil>>>;
struct CoreFrames<'t>(&'t mut LinkCore);
impl DispatchRun<Frames, CoreFrame, u32, u64> for CoreFrames<'_> {
    fn dispatch_run(&mut self, frame: CoreFrame, by: u32) -> u64 {
        self.0.rx += frame.0 * by;
        u64::from(self.0.rx)
    }
}
struct LossFrames<'t>(&'t mut SentRecords);
impl DispatchRun<Frames, LossFrame, u32, u64> for LossFrames<'_> {
    fn dispatch_run(&mut self, frame: LossFrame, by: u32) -> u64 {
        self.0.sent.push(frame.0 * u64::from(by));
        self.0.sent.len() as u64
    }
}
impl Contribute<Frames> for Core {
    type Needs = HList![Mut<LinkCore>];
    type Piece<'t> = CoreFrames<'t>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> CoreFrames<'t> {
        CoreFrames(view.head)
    }
}
impl Contribute<Frames> for Loss {
    type Needs = HList![Mut<SentRecords>];
    type Piece<'t> = LossFrames<'t>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> LossFrames<'t> {
        LossFrames(view.head)
    }
}
impl Contribute<Frames> for Watch {
    type Needs = HNil;
    type Piece<'t> = ();
    fn assemble<'t, P: Pos>(_: View<'t, Self::Needs, P>) -> Self::Piece<'t> {}
}

/// What the two families did at link 1 of `$root`, whose full key is
/// `$at`: the count, each frame's answer, and the count again.
macro_rules! contributions {
    ($root:expr, $at:expr) => {{
        macro_rules! count {
            () => {
                lend!(
                    $root,
                    LinkPath,
                    HList![Contributed<Counts, Parts>],
                    $at,
                    |hlist_pat![mut pieces]| {
                        let mut n = 0;
                        EachPiece::<Counts, u32>::each_piece(&mut pieces, &mut n);
                        n
                    }
                )
            };
        }
        let before = count!();
        let frames = lend!(
            $root,
            LinkPath,
            HList![Contributed<Frames, Parts>],
            $at,
            |hlist_pat![mut pieces]| {
                let mut take = |frame: FrameSum, by| {
                    DispatchSum::<Frames, FrameSum, u32, u64>::dispatch_sum(&mut pieces, frame, by)
                };
                (
                    take(Coproduct::Inl(CoreFrame(2)), 3),
                    take(Coproduct::Inr(Coproduct::Inl(LossFrame(5))), 3),
                    take(Coproduct::Inl(CoreFrame(1)), 1),
                )
            }
        );
        let after = count!();
        (before, frames, after)
    }};
}

#[test]
fn contributed() {
    let mut root = tree_one_as_two();
    let one = contributions!(&mut root, hlist![PathKey(10), LinkId(1)]);
    let mut root = tree_two();
    let two = contributions!(&mut root, hlist![LinkId(1)]);
    // Before: (0 + 1) * 10 and no record; after: the link counted 7 and
    // one record.
    same_on_both(one, two, (Some(10), Some((6, 1, 7)), Some(81)));
}

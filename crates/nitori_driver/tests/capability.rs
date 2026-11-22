//! Capabilities: composed (a capability in a capability, a capability in
//! a part's contribution) and contributed (every part's piece, dispatched
//! by a sum or run in turn; a piece of a layer that is not there).
#![feature(impl_trait_in_assoc_type, yield_expr)]
mod common;

use core::pin::pin;
use std::task::{Context, Waker};

use common::{net::*, new};
use frunk_core::{
    HList,
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil, Selector},
    hlist_pat,
};
use nitori_call::{LentAccess, call};
use nitori_driver::{
    call::{
        CallFamily, CallKind, CallStep, Composition, LoanNeeds, PartsOf, ShareCalls, poll, start,
    },
    capability::{
        Cap, Capability, Contribute, Contributed, DispatchRun, DispatchSum, EachPiece, EachRun,
        PartNeeds, PieceList,
    },
    composition,
    need::{At, Here, Kind, Mut, Ref, ToChild, Up},
    structure::{End, Node, NodePath, Share, Step, insert, remove},
};

// ---------------------------------------------------------------- family 1: frames, by the sum

/// The family: every part's piece that takes the part's frames.
pub struct FrameDispatch;
/// A frame of the type `T` and its value.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame<const T: u8>(pub u64);
/// The frames of a part, as the wire format defines them.
pub trait FrameSetPart {
    type Run;
}
/// The frames of a list of parts: one alternative per part.
pub trait FrameSet {
    type Sum;
}
impl FrameSet for HNil {
    type Sum = CNil;
}
impl<H: FrameSetPart, T: FrameSet> FrameSet for HCons<H, T> {
    type Sum = Coproduct<H::Run, T::Sum>;
}
/// Reading a frame of a sum from a frame received.
pub trait Decode: Sized {
    fn decode(raw: RawFrame) -> Option<Self>;
}
impl Decode for CNil {
    fn decode(_: RawFrame) -> Option<CNil> {
        None
    }
}
impl<H: Decode, T: Decode> Decode for Coproduct<H, T> {
    fn decode(raw: RawFrame) -> Option<Self> {
        H::decode(raw)
            .map(Coproduct::Inl)
            .or_else(|| T::decode(raw).map(Coproduct::Inr))
    }
}
impl<const T: u8> Decode for Frame<T> {
    fn decode(raw: RawFrame) -> Option<Self> {
        (raw.0 == T).then_some(Frame(raw.1))
    }
}
/// The layer a frame is for is not there.
#[derive(Debug, PartialEq, Eq)]
pub struct Absent;
type Dispatched = Result<(), Absent>;
/// A piece over one counter: frames of the type `T` are added in.
pub struct Sink<'w, const T: u8>(pub &'w mut u64);
impl<const T: u8> DispatchRun<FrameDispatch, Coproduct<Frame<T>, CNil>, (), Dispatched>
    for Sink<'_, T>
{
    fn dispatch_run(&mut self, item: Coproduct<Frame<T>, CNil>, _: ()) -> Dispatched {
        match item {
            Coproduct::Inl(f) => *self.0 += f.0,
            Coproduct::Inr(n) => match n {},
        }
        Ok(())
    }
}
/// A piece in a layer that may not be there: its frames are refused then.
impl<R, P: DispatchRun<FrameDispatch, R, (), Dispatched>>
    DispatchRun<FrameDispatch, R, (), Dispatched> for Option<P>
{
    fn dispatch_run(&mut self, item: R, arg: ()) -> Dispatched {
        self.as_mut().ok_or(Absent)?.dispatch_run(item, arg)
    }
}
/// What a call that dispatches frames asks of the composition.
pub trait DispatchesFrames:
    Composition<
    Parts: FrameSet<Sum: Decode>
               + for<'w> PieceList<
        FrameDispatch,
        Pieces<'w>: DispatchSum<FrameDispatch, FramesOf<Self>, (), Dispatched>,
    >,
>
{
}
impl<Q> DispatchesFrames for Q where
    Q: Composition<
        Parts: FrameSet<Sum: Decode>
                   + for<'w> PieceList<
            FrameDispatch,
            Pieces<'w>: DispatchSum<FrameDispatch, FramesOf<Q>, (), Dispatched>,
        >,
    >
{
}
pub type FramesOf<Q> = <PartsOf<Q> as FrameSet>::Sum;

// ---------------------------------------------------------------- family 2: sources, in turn

/// The family: every part's piece that has something to send.
pub struct Sources;
/// A piece over one value, read.
pub struct Counter<'w>(pub &'static str, pub &'w u64);
/// What a call asks every piece: names and values, into its own locals.
pub struct Ask<'l> {
    pub names: &'l mut Vec<&'static str>,
    pub total: &'l mut u64,
}
impl<'l> EachRun<Sources, Ask<'l>> for Counter<'_> {
    fn each_run(&mut self, ask: &mut Ask<'l>) {
        ask.names.push(self.0);
        *ask.total += *self.1;
    }
}
impl<'l, P: EachRun<Sources, Ask<'l>>> EachRun<Sources, Ask<'l>> for Option<P> {
    fn each_run(&mut self, ask: &mut Ask<'l>) {
        match self {
            Some(p) => p.each_run(ask),
            None => ask.names.push("absent"),
        }
    }
}

// ---------------------------------------------------------------- composed capabilities

/// Writes: the keys, shared, and the route's state, exclusive.
pub struct WriterCap;
pub struct Writer<V>(V);
impl Capability for WriterCap {
    type Needs = HList![
        At<Kind<Connection>, Here, Ref<Keys>>,
        At<Up<0>, Here, Mut<RouteState>>
    ];
    type View<V> = Writer<V>;
    fn assemble<V>(needs: V) -> Writer<V> {
        Writer(needs)
    }
}
impl Writer<HList![&Keys, &mut RouteState]> {
    pub fn write(&mut self) {
        let hlist_pat![keys, state] = &mut self.0;
        state.0 += keys.0;
    }
}
/// A capability with a capability in it: the writer and the timer's RTT.
pub struct SendCap;
pub struct Send<V>(V);
impl Capability for SendCap {
    type Needs = HList![Cap<WriterCap>, At<Up<0>, ToChild<Timer>, Ref<Rtt>>];
    type View<V> = Send<V>;
    fn assemble<V>(needs: V) -> Send<V> {
        Send(needs)
    }
}
impl<W> Send<HList![W, Option<&Rtt>]> {
    pub fn writer(&mut self) -> &mut W {
        &mut self.0.head
    }
    pub fn rtt(&self) -> Option<u64> {
        self.0.tail.head.map(|r| r.0)
    }
}
/// A capability in a part's contribution: the queue, shared.
pub struct QueueCap;
pub struct QueueView<V>(V);
impl Capability for QueueCap {
    type Needs = HList![At<Kind<Connection>, Here, Ref<Queue>>];
    type View<V> = QueueView<V>;
    fn assemble<V>(needs: V) -> QueueView<V> {
        QueueView(needs)
    }
}

// ---------------------------------------------------------------- calls

/// On a path: every frame received, dispatched by the sum.
pub struct Receive;
impl<Q: DispatchesFrames> CallKind<Q> for Receive {
    type Needs = HList![
        At<Up<0>, Here, Mut<Inbox>>,
        At<Kind<Connection>, Here, Ref<Keys>>,
        Contributed<FrameDispatch>,
    ];
    type Op = Receiving;
    fn start() -> Receiving {
        Receiving::new()
    }
}
#[call]
pub async fn receiving<W: CallFamily<C = Receive, Q: DispatchesFrames>>(
    host: &mut impl LentAccess<Family = W>,
) {
    host.with(|mut view| {
        let hlist_pat![inbox, keys, pieces] = view.needs();
        assert_eq!(keys.0, 7);
        let mut left = Vec::new();
        for raw in inbox.0.drain(..) {
            match FramesOf::<W::Q>::decode(raw) {
                Some(frame) => {
                    if pieces.dispatch_sum(frame, ()).is_err() {
                        left.push((0, raw.0.into()));
                    }
                }
                None => left.push((0xff, raw.0.into())),
            }
        }
        inbox.0 = left;
    });
}

/// On a route: the composed capability with one in it, a component of the
/// path above, and every part's source in turn into a visitor of locals.
pub struct RouteSend;
impl<Q: Composition> CallKind<Q> for RouteSend
where
    PartsOf<Q>: for<'w> PieceList<Sources, Pieces<'w>: for<'l> EachPiece<Sources, Ask<'l>>>,
{
    type Needs = HList![
        Cap<SendCap>,
        At<Up<1>, Here, Ref<PathState>>,
        Contributed<Sources>,
    ];
    type Op = RouteSending;
    type Yield = (Vec<&'static str>, u64, Option<u64>);
    fn start() -> RouteSending {
        RouteSending::new()
    }
}
#[call(yields = (Vec<&'static str>, u64, Option<u64>))]
pub async fn route_sending<W: CallFamily<C = RouteSend>>(host: &mut impl LentAccess<Family = W>)
where
    PartsOf<<W as CallFamily>::Q>:
        for<'w> PieceList<Sources, Pieces<'w>: for<'l> EachPiece<Sources, Ask<'l>>>,
{
    let report = host.with(|mut view| {
        let hlist_pat![send, path, sources] = view.needs();
        send.writer().write();
        let mut names = Vec::new();
        let mut total = path.0;
        sources.each_piece(&mut Ask {
            names: &mut names,
            total: &mut total,
        });
        (names, total, send.rtt())
    });
    yield report;
}

// ---------------------------------------------------------------- parts

macro_rules! part {
    ($part:ident { $($kind:ty => [$($comp:ty),*] calls [$($call:ty),*]),* $(,)? }) => {
        pub struct $part;
        $(
            impl Share<$kind> for $part { type Data = HList![$($comp),*]; }
            impl ShareCalls<$kind> for $part { type Calls = HList![$($call),*]; }
        )*
    };
}
part!(Base {
    Connection => [Keys, Clock] calls [],
    Session => [] calls [],
    Peer => [Budget] calls [],
    Path => [PathState, Inbox] calls [Receive],
    Route => [RouteState] calls [RouteSend],
    Timer => [Rtt] calls [],
});
part!(Streamed {
    Connection => [] calls [],
    Session => [SessionRecv] calls [],
    Peer => [] calls [],
    Path => [] calls [],
    Route => [] calls [],
    Timer => [] calls [],
});
part!(Dgram {
    Connection => [Queue] calls [],
    Session => [] calls [],
    Peer => [] calls [],
    Path => [] calls [],
    Route => [] calls [],
    Timer => [] calls [],
});

impl FrameSetPart for Base {
    type Run = Coproduct<Frame<2>, CNil>;
}
impl FrameSetPart for Streamed {
    type Run = Coproduct<Frame<8>, CNil>;
}
impl FrameSetPart for Dgram {
    type Run = CNil;
}
impl Contribute<FrameDispatch> for Base {
    type Needs = HList![At<Kind<Path>, Here, Mut<PathState>>];
    type Piece<'w> = Sink<'w, 2>;
    fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> Sink<'_, 2> {
        let hlist_pat![state] = needs;
        Sink(&mut state.0)
    }
}
impl Contribute<FrameDispatch> for Streamed {
    type Needs = HList![At<Kind<Connection>, ToChild<Session>, Mut<SessionRecv>>];
    type Piece<'w> = Option<Sink<'w, 8>>;
    fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> Option<Sink<'_, 8>> {
        let hlist_pat![recv] = needs;
        recv.map(|r| Sink(&mut r.0))
    }
}
impl Contribute<FrameDispatch> for Dgram {
    type Needs = HNil;
    type Piece<'w> = ();
    fn assemble(_: <HNil as PartNeeds>::View<'_>) -> Self::Piece<'_> {}
}
impl Contribute<Sources> for Base {
    type Needs = HList![At<Kind<Connection>, Here, Ref<Clock>>];
    type Piece<'w> = Counter<'w>;
    fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> Counter<'_> {
        let hlist_pat![clock] = needs;
        Counter("base", &clock.0)
    }
}
impl Contribute<Sources> for Streamed {
    type Needs = HList![At<Kind<Connection>, ToChild<Session>, Ref<SessionRecv>>];
    type Piece<'w> = Option<Counter<'w>>;
    fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> Option<Counter<'_>> {
        let hlist_pat![recv] = needs;
        recv.map(|r| Counter("streamed", &r.0))
    }
}
impl Contribute<Sources> for Dgram {
    type Needs = HList![Cap<QueueCap>];
    type Piece<'w> = Counter<'w>;
    fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> Counter<'_> {
        let hlist_pat![QueueView(hlist_pat![queue])] = needs;
        Counter("dgram", &queue.0)
    }
}

pub struct Q2;
pub struct Q3;
composition!(impl for Q2: Connection, HList![Base, Streamed]);
composition!(impl for Q3: Connection, HList![Base, Streamed, Dgram]);

fn component<X, I, D: Selector<X, I>>(d: &mut D) -> &mut X {
    d.get_mut()
}
fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

macro_rules! build {
    ($q:ty) => {{
        let mut root: Node<Connection, <$q as Composition>::Parts> = new();
        component::<Keys, _, _>(&mut root.data.head).0 = 7;
        component::<Clock, _, _>(&mut root.data.head).0 = 1;
        let session: SessionAt = insert(&mut root, End, (), new()).ok().unwrap();
        let _: PeerAt = insert(&mut root, End, PeerId(7), new()).ok().unwrap();
        let p0: PathAt = insert(&mut root, End, PathId(0), new()).ok().unwrap();
        let route: RouteAt = insert(&mut root, p0, PeerId(7), new()).ok().unwrap();
        (root, session, p0, route)
    }};
}

#[test]
fn frames_dispatched_by_the_sum() {
    let (mut root, session, p0, _) = build!(Q3);
    let mut cx = cx();
    component::<Inbox, _, _>(&mut p0.node_mut(&mut root).unwrap().data.head).0 =
        vec![(2, 5), (8, 3), (9, 1), (2, 1)];
    let mut call = pin!(start::<Q3, Receive, _, _>(&p0));
    assert_eq!(
        poll::<Q3, Receive, _, _>(&mut root, p0, call.as_mut(), &mut cx),
        Ok(CallStep::Finished)
    );
    let path = &p0.node(&root).unwrap().data.head;
    assert_eq!(path.head, PathState(6));
    assert_eq!(path.tail.head.0, [(0xff, 9)]);
    assert_eq!(
        session.node(&root).unwrap().data.tail.head.head,
        SessionRecv(3)
    );

    // Without the session, its part's frames are refused.
    remove(&mut root, session).unwrap();
    component::<Inbox, _, _>(&mut p0.node_mut(&mut root).unwrap().data.head).0 = vec![(8, 3)];
    let mut call = pin!(start::<Q3, Receive, _, _>(&p0));
    assert_eq!(
        poll::<Q3, Receive, _, _>(&mut root, p0, call.as_mut(), &mut cx),
        Ok(CallStep::Finished)
    );
    assert_eq!(p0.node(&root).unwrap().data.head.tail.head.0, [(0, 8)]);
}

#[test]
fn composed_capabilities_and_pieces_in_turn() {
    let (mut root, session, p0, route) = build!(Q3);
    let mut cx = cx();
    let timer: TimerAt = Step::new(
        route.handle,
        Step::new(route.rest.handle, Step::new((), End)),
    );
    let _ = insert(
        &mut root,
        route,
        (),
        new::<Timer, <Q3 as Composition>::Parts>(),
    );
    component::<Rtt, _, _>(&mut timer.node_mut(&mut root).unwrap().data.head).0 = 33;
    component::<PathState, _, _>(&mut p0.node_mut(&mut root).unwrap().data.head).0 = 10;
    root.data.tail.tail.head.head.0 = 100;
    session.node_mut(&mut root).unwrap().data.tail.head.head.0 = 20;
    let mut call = pin!(start::<Q3, RouteSend, _, _>(&route));
    assert_eq!(
        poll::<Q3, RouteSend, _, _>(&mut root, route, call.as_mut(), &mut cx),
        Ok(CallStep::Yielded((
            vec!["base", "streamed", "dgram"],
            10 + 1 + 20 + 100,
            Some(33)
        )))
    );
    assert_eq!(route.node(&root).unwrap().data.head.head, RouteState(7));

    // Two parts, no session, no timer: the same call's source.
    let (mut root, session, _, route) = build!(Q2);
    remove(&mut root, session).unwrap();
    let mut call = pin!(start::<Q2, RouteSend, _, _>(&route));
    assert_eq!(
        poll::<Q2, RouteSend, _, _>(&mut root, route, call.as_mut(), &mut cx),
        Ok(CallStep::Yielded((vec!["base", "absent"], 1, None)))
    );
}

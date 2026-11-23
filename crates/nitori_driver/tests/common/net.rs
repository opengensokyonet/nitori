//! A connection with several layers. The connection has a session (at
//! most one), peers by peer ID and paths by path ID; a path has routes by
//! peer ID, and each route links to the connection's peer of its own key;
//! a route has a timer (at most one).
use frunk_core::{
    HList,
    hlist::HNil,
    indices::{Here as I0, There},
};
use std::cell::RefCell;

use nitori_driver::{
    request::{Edit, Editor, Request},
    structure::{AtMostOne, ByOwnKey, Child, End, NodeKind, Share, SlotVec, Step},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathId(pub u32);

pub struct Connection;
pub struct Session;
pub struct Peer;
pub struct Path;
pub struct Route;
pub struct Timer;

impl NodeKind for Connection {
    type Children = HList![
        Child<Session, AtMostOne>,
        Child<Peer, SlotVec<PeerId>>,
        Child<Path, SlotVec<PathId>>,
    ];
    type Linked = HNil;
    type Request = Noted;
}

thread_local! {
    static NOTED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}
/// What the connection is asked: a number, noted and nothing changed.
pub struct Noted(pub u64);
impl Request<Connection> for Noted {
    fn apply<E: Editor<Connection>>(self, _: Edit<'_, E, Connection>) {
        NOTED.with(|n| n.borrow_mut().push(self.0));
    }
}
/// The numbers noted since the last time.
pub fn noted() -> Vec<u64> {
    NOTED.with(|n| core::mem::take(&mut *n.borrow_mut()))
}
impl NodeKind for Session {
    type Children = HNil;
    type Linked = HNil;
}
impl NodeKind for Peer {
    type Children = HNil;
    type Linked = HNil;
}
impl NodeKind for Path {
    type Children = HList![Child<Route, SlotVec<PeerId>>];
    type Linked = HNil;
}
/// A route links to the connection's peer of its own key.
impl NodeKind for Route {
    type Children = HList![Child<Timer, AtMostOne>];
    type Linked = HList![ByOwnKey<Peer>];
}
impl nitori_driver::structure::LinkedStart<Peer> for Route {
    fn linked_start(_: &mut ()) {}
}
impl NodeKind for Timer {
    type Children = HNil;
    type Linked = HNil;
}

pub type I1 = There<I0>;
pub type I2 = There<I1>;
/// Connection / session.
pub type SessionAt = Step<I0, (), End>;
/// Connection / peer.
pub type PeerAt = Step<I1, usize, End>;
/// Connection / path.
pub type PathAt = Step<I2, usize, End>;
/// Connection / path / route.
pub type RouteAt = Step<I2, usize, Step<I0, usize, End>>;
/// Connection / path / route / timer.
pub type TimerAt = Step<I2, usize, Step<I0, usize, Step<I0, (), End>>>;

macro_rules! components {
    ($($c:ident),* $(,)?) => {$(
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct $c(pub u64);
    )*};
}
components!(
    Keys,
    Clock,
    Budget,
    PathStreams,
    PathState,
    RouteState,
    Rtt,
    SessionRecv,
    Streams,
    StreamsRoute,
    Queue,
    Extra,
    ExtraRoute,
);
/// A frame as received: type and value.
pub type RawFrame = (u8, u64);
/// Frames received on a path, not yet handled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inbox(pub Vec<RawFrame>);
/// A path's own path ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathIdent(pub u32);
/// Acknowledgements received on a path: the path acknowledged and a value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathAcks(pub Vec<(PathId, u64)>);
/// On the connection: paths and peers to update by key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub paths: Vec<(PathId, u64)>,
    pub peers: Vec<(PeerId, u64)>,
}

/// Declare a part: its share in each kind.
macro_rules! part {
    ($part:ident { $($kind:ty => [$($comp:ty),*]),* $(,)? }) => {
        pub struct $part;
        $( impl Share<$kind> for $part { type Data = HList![$($comp),*]; } )*
    };
}

part!(Core {
    Connection => [Keys, Clock, Probe],
    Session => [],
    Peer => [Budget],
    Path => [PathState, Inbox, PathIdent, PathAcks],
    Route => [RouteState],
    Timer => [Rtt],
});
part!(Streaming {
    Connection => [Streams],
    Session => [SessionRecv],
    Peer => [],
    Path => [PathStreams],
    Route => [StreamsRoute],
    Timer => [],
});
part!(Datagrams {
    Connection => [Queue],
    Session => [],
    Peer => [],
    Path => [],
    Route => [],
    Timer => [],
});
part!(Additions {
    Connection => [Extra],
    Session => [],
    Peer => [],
    Path => [],
    Route => [ExtraRoute],
    Timer => [],
});

/// Two parts.
pub type P2 = HList![Core, Streaming];
/// Four parts.
pub type P4 = HList![Core, Streaming, Datagrams, Additions];

//! An HTTP connection: a control stream (at most one) and request streams
//! by stream ID, each with a body (at most one).
use frunk_core::{
    HList,
    hlist::HNil,
    indices::{Here as I0, There},
};
use nitori_driver::structure::{AtMostOne, Child, End, NodeKind, Share, SlotVec, Step};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamId(pub u64);

pub struct HttpConnection;
pub struct Control;
pub struct Stream;
pub struct Body;

impl NodeKind for HttpConnection {
    type Children = HList![Child<Control, AtMostOne>, Child<Stream, SlotVec<StreamId>>];
    type Linked = HNil;
}
impl NodeKind for Control {
    type Children = HNil;
    type Linked = HNil;
}
impl NodeKind for Stream {
    type Children = HList![Child<Body, AtMostOne>];
    type Linked = HNil;
}
impl NodeKind for Body {
    type Children = HNil;
    type Linked = HNil;
}

/// Connection / control stream.
pub type ControlAt = Step<I0, (), End>;
/// Connection / request stream.
pub type StreamAt = Step<There<I0>, usize, End>;
/// Connection / request stream / body.
pub type BodyAt = Step<There<I0>, usize, Step<I0, (), End>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings(pub u64);
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Headers(pub Vec<&'static str>);
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Received(pub u64);
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limits(pub u64);

/// The one part of this structure.
pub struct Http;
impl Share<HttpConnection> for Http {
    type Data = HList![Settings, Limits];
}
impl Share<Control> for Http {
    type Data = HList![Settings];
}
impl Share<Stream> for Http {
    type Data = HList![Headers];
}
impl Share<Body> for Http {
    type Data = HList![Received];
}
pub type H1 = HList![Http];

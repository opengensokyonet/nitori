//! Requests: what a call, a component's operation or a reaction asks a
//! node to change in its own subtree.
//!
//! Each kind of node declares what it can be asked
//! ([`NodeKind::Request`], `!` for nothing). A request names
//! its node by its key address from the root, whose type says the kind it
//! leads to ([`NodeRequest`], [`KeyAddress`]), so a request for another
//! kind does not compile. It is put in the lending's box with
//! [`Requests`](crate::need::Requests)`<NodeRequest<Root, Kind>>`: the
//! context of a composition holds a box for every kind of its structure.
//!
//! The driver hands a request to its node's kind: the kind names its
//! handler ([`NodeKind::Handler`]), which gets an edit handle at the
//! node ([`Edit`]) and changes the node's own subtree with it.
use core::fmt;

use frunk_core::hlist::HNil;

use crate::{
    driver::{Edit, HandlerFor},
    need::Need,
    structure::{KeyAddress, NodeKind},
};

/// A request for the node of the kind `K` at the key address `at` from
/// the root, of the kind `R`.
pub struct NodeRequest<R: NodeKind, K: NodeKind> {
    /// The node it is for.
    pub at: KeyAddress<R, K>,
    /// What it asks.
    pub request: K::Request,
}
impl<R: NodeKind, K: NodeKind> NodeRequest<R, K> {
    /// The request `request` for the node at `at`, an address that leads
    /// to a node of the kind `K`.
    pub fn new(at: KeyAddress<R, K>, request: K::Request) -> Self {
        Self { at, request }
    }
}
impl<R: NodeKind, K: NodeKind<Request: PartialEq>> PartialEq for NodeRequest<R, K> {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.request == other.request
    }
}
impl<R: NodeKind, K: NodeKind<Request: fmt::Debug>> fmt::Debug for NodeRequest<R, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeRequest")
            .field("at", &self.at)
            .field("request", &self.request)
            .finish()
    }
}

/// What handles the requests of the nodes of the kind `K`, under a
/// composition whose priority is `Pr`: a kind names it as its
/// [`NodeKind::Handler`].
///
/// The handler gets an edit handle at the node the request is for, which
/// reaches the node's own subtree only, and lends its `Needs` there
/// ([`Edit::lend`]), as often as it likes, between its changes. The
/// composition proves the needs at the kind's nodes, and that the
/// handler's priority is the composition's.
///
/// A root that is asked to end, for any composition at the priority
/// `u8`:
///
/// ```
/// #![feature(impl_trait_in_assoc_type)]
/// use nitori_driver::{
///     __private::calls::{StillClock, Total},
///     component::Part,
///     composition,
///     driver::{Edit, HandlerFor},
///     frunk_core::HList,
///     id::Numbered,
///     request::HandleRequest,
///     structure::{NodeKind, Share},
/// };
/// #[derive(Numbered)]
/// struct Top;
/// impl NodeKind for Top {
///     type Children = HList![];
///     type Request = u8;
///     type Handler = Closing;
/// }
/// struct Closing;
/// impl HandleRequest<Top, u8> for Closing {
///     type Needs = HList![];
///     fn handle<P: HandlerFor<Top, Self, u8>>(
///         edit: Edit<'_, Top, P>,
///         _: u8,
///     ) {
///         edit.end_self(());
///     }
/// }
/// struct One;
/// impl<L> Part<L> for One {
///     type Args = ();
///     type Shares = HList![Share<Top, HList![Total]>];
/// }
/// struct Q;
/// composition!(impl for Q: Top, HList![One], StillClock, u8);
/// ```
///
/// The same handler in a composition at another priority does not
/// compile:
///
/// ```compile_fail,E0277
/// # #![feature(impl_trait_in_assoc_type)]
/// # use nitori_driver::{
/// #     __private::calls::{StillClock, Total},
/// #     component::Part,
/// #     composition,
/// #     driver::{Edit, HandlerFor},
/// #     frunk_core::HList,
/// #     id::Numbered,
/// #     request::HandleRequest,
/// #     structure::{NodeKind, Share},
/// # };
/// # #[derive(Numbered)]
/// # struct Top;
/// # impl NodeKind for Top {
/// #     type Children = HList![];
/// #     type Request = u8;
/// #     type Handler = Closing;
/// # }
/// # struct Closing;
/// # impl HandleRequest<Top, u8> for Closing {
/// #     type Needs = HList![];
/// #     fn handle<P: HandlerFor<Top, Self, u8>>(
/// #         edit: Edit<'_, Top, P>,
/// #         _: u8,
/// #     ) {
/// #         edit.end_self(());
/// #     }
/// # }
/// # struct One;
/// # impl<L> Part<L> for One {
/// #     type Args = ();
/// #     type Shares = HList![Share<Top, HList![Total]>];
/// # }
/// struct Q;
/// composition!(impl for Q: Top, HList![One], StillClock, u16);
/// ```
///
/// A kind with requests and no handler does not compile either: its
/// handler is [`NoRequests`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not handle the requests of the node kind `{K}` at the priority `{Pr}`",
    label = "the handler of a kind of the structure",
    note = "a kind with requests names its handler (`type Handler`), which implements `HandleRequest<Kind, Priority>` for the composition's priority; the default, `NoRequests`, handles only a kind whose `Request` is `!`"
)]
pub trait HandleRequest<K: NodeKind, Pr>: 'static {
    /// What the edit handle lends, at the node the request is for.
    type Needs: Need;
    /// Handle `request`, for the node `edit` is at.
    fn handle<P: HandlerFor<K, Self, Pr>>(edit: Edit<'_, K, P>, request: K::Request);
}

/// The handler of a kind that cannot be asked anything: its `Request`
/// is `!`.
pub struct NoRequests;
impl<K: NodeKind<Request: EmptyRequest>, Pr> HandleRequest<K, Pr> for NoRequests {
    type Needs = HNil;
    fn handle<P: HandlerFor<K, Self, Pr>>(_: Edit<'_, K, P>, request: K::Request) {
        request.never()
    }
}
/// The request type `Self` has no value: a kind with it is asked
/// nothing.
#[diagnostic::on_unimplemented(
    message = "a node kind whose `Request` is `{Self}` has the handler `NoRequests`",
    label = "the requests of a kind with no handler",
    note = "`NoRequests`, the default handler, handles only a kind whose `Request` is `!`; a kind with requests names its handler, `type Handler`, which implements `HandleRequest<Kind, Priority>`"
)]
pub trait EmptyRequest {
    /// It never is.
    fn never(self) -> !;
}
impl EmptyRequest for ! {
    fn never(self) -> ! {
        self
    }
}

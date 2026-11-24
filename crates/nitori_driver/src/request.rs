//! Requests: what a call, a component's operation or a reaction asks a
//! node to change in its own subtree.
//!
//! Each kind of node declares what it can be asked
//! ([`NodeKind::Request`], `Infallible` for nothing). A request names
//! its node by its key address from the root, whose type says the kind it
//! leads to ([`NodeRequest`], [`KeyAddress`]), so a request for another
//! kind does not compile. It is put in the lending's box with
//! [`Requests`](crate::need::Requests)`<NodeRequest<Root, Kind>>`: the
//! context of a composition holds a box for every kind of its structure.
use core::fmt;

use crate::structure::{KeyAddress, NodeKind};

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

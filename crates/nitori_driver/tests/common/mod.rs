//! Two structures the tests share: a connection with paths, routes and
//! the entries the routes link to, and an HTTP connection with streams.
#![allow(dead_code)]

pub mod http;
pub mod net;

use nitori_driver::structure::{KindFor, Node};

/// A node with every component at its default.
pub fn new<K: KindFor<P>, P>() -> Node<K, P>
where
    K::Data: Default,
{
    Node::new(Default::default())
}

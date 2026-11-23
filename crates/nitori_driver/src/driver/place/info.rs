//! What the driver keeps of a place and of each component at it.

#[cfg(doc)]
use crate::event::{NodeEnding, NodeStarted};
#[cfg(doc)]
use crate::structure::NodeKind;

use alloc::vec::Vec;
use core::any::TypeId;

use crate::event::Subscription;

/// A component of a place, as the driver keeps it.
#[derive(Debug)]
pub struct CompInfo {
    /// Its type.
    pub type_id: TypeId,
    /// Whether it has a deadline.
    pub timed: bool,
    /// Whether it acts before the calls.
    pub acts_before_calls: bool,
    /// What it subscribes to.
    pub subscriptions: Vec<Subscription>,
    /// The components its hooks take exclusive.
    pub writes: Vec<TypeId>,
    /// The components its `build` takes exclusive at the parent.
    pub build_writes: Vec<TypeId>,
}
/// A place, as the driver keeps it.
#[derive(Debug)]
pub struct PlaceInfo {
    /// Its path type.
    pub path: TypeId,
    /// The parent's path type; none for the root.
    pub parent: Option<TypeId>,
    /// Its position among the children of its parent's kind; none for the
    /// root.
    pub index: Option<usize>,
    /// Its kind.
    pub kind: TypeId,
    /// The kinds its kind links to ([`NodeKind::Linked`]).
    pub links: Vec<TypeId>,
    /// Its positions up to the root, as subscriptions name places.
    pub pre: TypeId,
    /// How deep its nodes are; the root at 0.
    pub depth: usize,
    /// [`NodeStarted`] of its kind.
    pub started: TypeId,
    /// [`NodeEnding`] of its kind.
    pub ending: TypeId,
    /// Its components, part by part.
    pub comps: Vec<CompInfo>,
}

//! What the driver calls on every component of a node: its hooks.
//!
//! A component is the `Self` of [`Hooks<O, L, K>`](Hooks): `O` is the part
//! whose share it is in, `L` the whole list of parts, `K` the node kind.
//! A component may be generic over the list, and answer differently by
//! which parts are in it. The part `O` comes before `L`, so the crate
//! that defines a part can write the hooks of a generic type of another
//! crate (`impl<X, L, K> Hooks<MyPart, L, K> for TheirType<X>`). A bare
//! type parameter the host chooses cannot be `Self` of a foreign trait
//! (E0210); such a component is kept in [`Held<X>`].
//!
//! Every part has construction arguments, whose type may depend on the
//! whole list ([`Part<L>`](Part)); the driver puts them in one list
//! ([`ArgsOf<L>`](ArgsOf)) and gives every hook all of it. A component
//! reads what it needs through a trait of its own crate on that list,
//! answered part by part.
//!
//! The driver calls the hooks of a share's components one by one
//! ([`ShareHooks`]) and of a node's shares part by part ([`DataHooks`]),
//! written once here.
//!
//! ```
//! use frunk_core::{HList, hlist::{HCons, HNil}};
//! use nitori_driver::{
//!     hooks::{ArgsOf, Held, Hooks, Part, PartList, build_node},
//!     structure::{NodeKind, Share},
//! };
//!
//! struct Root;
//! impl NodeKind for Root {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! /// A part and its arguments.
//! struct Base;
//! struct BaseArgs {
//!     window: u64,
//! }
//! impl<L> Part<L> for Base {
//!     type Args = BaseArgs;
//! }
//! /// An extension and its arguments.
//! struct Extension;
//! struct ExtensionArgs {
//!     enabled: bool,
//! }
//! impl<L> Part<L> for Extension {
//!     type Args = ExtensionArgs;
//! }
//!
//! /// What a set of streams reads of the arguments, part by part.
//! trait StreamArgs {
//!     fn window(&self) -> Option<u64> {
//!         None
//!     }
//!     fn reliable(&self) -> bool {
//!         false
//!     }
//! }
//! impl StreamArgs for BaseArgs {
//!     fn window(&self) -> Option<u64> {
//!         Some(self.window)
//!     }
//! }
//! impl StreamArgs for ExtensionArgs {
//!     fn reliable(&self) -> bool {
//!         self.enabled
//!     }
//! }
//! /// The same of the whole list.
//! trait StreamArgsList {
//!     fn window(&self) -> Option<u64>;
//!     fn reliable(&self) -> bool;
//! }
//! impl StreamArgsList for HNil {
//!     fn window(&self) -> Option<u64> {
//!         None
//!     }
//!     fn reliable(&self) -> bool {
//!         false
//!     }
//! }
//! impl<A: StreamArgs, T: StreamArgsList> StreamArgsList for HCons<A, T> {
//!     fn window(&self) -> Option<u64> {
//!         self.head.window().or(self.tail.window())
//!     }
//!     fn reliable(&self) -> bool {
//!         self.head.reliable() || self.tail.reliable()
//!     }
//! }
//!
//! /// A component generic over the list of parts.
//! struct Streams {
//!     window: u64,
//!     reliable: bool,
//! }
//! impl<O, L: PartList<L, Args: StreamArgsList>, K> Hooks<O, L, K> for Streams {
//!     fn build(args: &ArgsOf<L>) -> Self {
//!         Streams {
//!             window: args.window().unwrap_or(0),
//!             reliable: args.reliable(),
//!         }
//!     }
//! }
//! impl Share<Root> for Base {
//!     type Data = HList![Streams];
//! }
//! impl Share<Root> for Extension {
//!     type Data = HNil;
//! }
//!
//! type Parts = HList![Base, Extension];
//! let args: ArgsOf<Parts> = frunk_core::hlist![BaseArgs { window: 10 }, ExtensionArgs { enabled: true }];
//! let root = build_node::<Root, Parts>(&args);
//! assert_eq!((root.data.head.head.window, root.data.head.head.reliable), (10, true));
//! ```
//!
//! A host's bare type is not a component; it goes in [`Held`]:
//!
//! ```compile_fail
//! use nitori_driver::hooks::{ArgsOf, Hooks, PartList};
//! trait Estimator {
//!     fn new() -> Self;
//! }
//! struct Base;
//! impl<X: Estimator, L: PartList<L>, K> Hooks<Base, L, K> for X {
//!     fn build(_: &ArgsOf<L>) -> Self {
//!         X::new()
//!     }
//! }
//! ```
//!
//! ```
//! use nitori_driver::hooks::{ArgsOf, Held, Hooks, PartList};
//! trait Estimator {
//!     fn new() -> Self;
//! }
//! struct Base;
//! impl<X: Estimator, L: PartList<L>, K> Hooks<Base, L, K> for Held<X> {
//!     fn build(_: &ArgsOf<L>) -> Self {
//!         Held(X::new())
//!     }
//! }
//! ```
use core::task::Waker;

use frunk_core::hlist::{HCons, HNil};

use crate::structure::{KindFor, Node};

/// The construction arguments of the part `Self` in a composition of the
/// parts `L`; their type may depend on the whole list.
pub trait Part<L> {
    /// The arguments.
    type Args;
}
/// The arguments of a list of parts `Self` in a composition of the parts
/// `L`: one per part, in order.
pub trait PartList<L> {
    /// The list of arguments.
    type Args;
}
impl<L> PartList<L> for HNil {
    type Args = HNil;
}
impl<L, P: Part<L>, Ps: PartList<L>> PartList<L> for HCons<P, Ps> {
    type Args = HCons<P::Args, Ps::Args>;
}
/// The arguments of every part of the list `L`.
pub type ArgsOf<L> = <L as PartList<L>>::Args;

/// Why a node's components learn that the driver went away before the
/// node ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortCause {
    /// The driver was dropped; every node is ended after this.
    Dropped,
    /// The driver panicked; the nodes are not ended, and a hook can only
    /// mark state and wake.
    Panicked,
}

/// The hooks of a component in the share of the part `O`, in a
/// composition of the parts `L`, in a node of the kind `K`.
///
/// Every hook but [`build`](Hooks::build) does nothing by default.
pub trait Hooks<O, L: PartList<L>, K>: Sized {
    /// Whether [`before_calls`](Hooks::before_calls) does anything.
    const ACTS_BEFORE_CALLS: bool = false;
    /// The component of a new node.
    fn build(args: &ArgsOf<L>) -> Self;
    /// Before any call at the start of a poll: in the first poll after
    /// the node is built and after `waker` was woken; nodes above first.
    fn before_calls(&mut self, waker: &Waker) {
        let _ = waker;
    }
    /// The driver went away before the node ended.
    fn aborted(&mut self, cause: AbortCause) {
        let _ = cause;
    }
    /// The node ends; the component goes.
    fn end(self) {}
}

/// A component whose type is a type parameter the host chooses, kept so
/// that a part can write its hooks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held<X>(pub X);

/// The hooks of every component of the share `Self` of the part `O`, one
/// by one.
pub trait ShareHooks<O, L: PartList<L>, K>: Sized {
    /// Whether any component acts before the calls.
    const ACTS_BEFORE_CALLS: bool;
    /// Every component of a new node.
    fn build(args: &ArgsOf<L>) -> Self;
    /// Every component that acts, before the calls.
    fn before_calls(&mut self, waker: &Waker);
    /// Every component: the driver went away.
    fn aborted(&mut self, cause: AbortCause);
    /// Every component: the node ends.
    fn end(self);
}
impl<O, L: PartList<L>, K> ShareHooks<O, L, K> for HNil {
    const ACTS_BEFORE_CALLS: bool = false;
    fn build(_: &ArgsOf<L>) -> Self {
        HNil
    }
    fn before_calls(&mut self, _: &Waker) {}
    fn aborted(&mut self, _: AbortCause) {}
    fn end(self) {}
}
impl<O, L: PartList<L>, K, H: Hooks<O, L, K>, T: ShareHooks<O, L, K>> ShareHooks<O, L, K>
    for HCons<H, T>
{
    const ACTS_BEFORE_CALLS: bool = H::ACTS_BEFORE_CALLS || T::ACTS_BEFORE_CALLS;
    fn build(args: &ArgsOf<L>) -> Self {
        HCons {
            head: H::build(args),
            tail: T::build(args),
        }
    }
    fn before_calls(&mut self, waker: &Waker) {
        if H::ACTS_BEFORE_CALLS {
            self.head.before_calls(waker);
        }
        self.tail.before_calls(waker);
    }
    fn aborted(&mut self, cause: AbortCause) {
        self.head.aborted(cause);
        self.tail.aborted(cause);
    }
    fn end(self) {
        self.head.end();
        self.tail.end();
    }
}

/// The hooks of every share of a node's data `Self`, the shares of the
/// parts `Ps` (the rest of `L`), part by part.
pub trait DataHooks<Ps, L: PartList<L>, K>: Sized {
    /// Whether any component acts before the calls.
    const ACTS_BEFORE_CALLS: bool;
    /// Every share of a new node.
    fn build(args: &ArgsOf<L>) -> Self;
    /// Every component that acts, before the calls.
    fn before_calls(&mut self, waker: &Waker);
    /// Every component: the driver went away.
    fn aborted(&mut self, cause: AbortCause);
    /// Every component: the node ends.
    fn end(self);
}
impl<L: PartList<L>, K> DataHooks<HNil, L, K> for HNil {
    const ACTS_BEFORE_CALLS: bool = false;
    fn build(_: &ArgsOf<L>) -> Self {
        HNil
    }
    fn before_calls(&mut self, _: &Waker) {}
    fn aborted(&mut self, _: AbortCause) {}
    fn end(self) {}
}
impl<O, Ps, L: PartList<L>, K, D, Ds> DataHooks<HCons<O, Ps>, L, K> for HCons<D, Ds>
where
    D: ShareHooks<O, L, K>,
    Ds: DataHooks<Ps, L, K>,
{
    const ACTS_BEFORE_CALLS: bool = D::ACTS_BEFORE_CALLS || Ds::ACTS_BEFORE_CALLS;
    fn build(args: &ArgsOf<L>) -> Self {
        HCons {
            head: D::build(args),
            tail: Ds::build(args),
        }
    }
    fn before_calls(&mut self, waker: &Waker) {
        self.head.before_calls(waker);
        self.tail.before_calls(waker);
    }
    fn aborted(&mut self, cause: AbortCause) {
        self.head.aborted(cause);
        self.tail.aborted(cause);
    }
    fn end(self) {
        self.head.end();
        self.tail.end();
    }
}

/// A new node of the kind `K` for the parts `L`, every component built
/// from `args`; it has no children.
pub fn build_node<K, L>(args: &ArgsOf<L>) -> Node<K, L>
where
    K: KindFor<L, Data: DataHooks<L, L, K>>,
    L: PartList<L>,
{
    Node::new(<K::Data as DataHooks<L, L, K>>::build(args))
}

/// End the node `node` of the kind `K`: every component's
/// [`end`](Hooks::end), part by part. Its children are ended before it
/// (see [`removal_order`](crate::structure::removal_order)) and are
/// dropped with it here.
pub fn end_node<K, L>(node: Node<K, L>)
where
    K: KindFor<L, Data: DataHooks<L, L, K>>,
    L: PartList<L>,
{
    node.data.end();
}

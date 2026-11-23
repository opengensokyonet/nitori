//! The structure of a tree: node kinds, the storage of their children,
//! nodes, typed paths and the driver's addresses.
//!
//! The code that uses the driver declares its structure. A node kind
//! ([`NodeKind`]) lists its children, each a kind and a storage kind
//! ([`Child`]). The kinds below a kind are declared the same way, so the
//! structure is a tree of kinds with one root. A kind may not be its own
//! child or descendant, and is at one place of the structure only.
//!
//! What a node holds comes from the parts: each part keeps a share
//! ([`Share`]) in each kind of node, and a node's data is the list of the
//! parts' shares ([`KindFor::Data`]). The parts do not enter the structure:
//! a typed path ([`Step`], [`End`]) and an address ([`Address`]) follow from
//! the kinds alone, whatever the parts are.
mod address;
mod edit;
mod kind;
mod node;
mod path;
mod storage;

pub use address::{Address, ChildAddresses, KeyAddress, KeyRoute, resolve_node};
pub use edit::{
    InsertError, LastChildren, LastKind, MoveError, NoParentError, OccupiedError, ParentChildren,
};
#[allow(unused_imports)]
pub(crate) use edit::{insert, move_to, remove};
pub use kind::{
    Child, ChildIndex, ChildList, ChildOfKind, ChildrenOf, HandleAt, KeyAt, KindAt, NoChild,
    NoStore, NodeKind, StorageAt,
};
pub use node::{
    ChildAt, ChildStores, FlatMut, FlatRef, KindFor, Node, RemainsList, RemainsOf, Share, Shares,
};
pub use path::{
    Append, End, FromAddress, KeyedPath, NodePath, ParentPath, PathFrom, SplitLast, Step,
};
pub use storage::{AtMostOne, ChildStorage, Occupied, SlotVec, SlotVecStore, Store, StoreOf};

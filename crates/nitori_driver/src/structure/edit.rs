//! Why a node could not be added under a key, or moved; and, for the
//! tests, putting a node in the tree by path.
use snafu::Snafu;

#[cfg(test)]
use super::{
    Append, ChildAt, ChildOfKind, ChildrenOf, HandleAt, KeyAt, KindFor, Node, NodePath, Occupied,
    PathFrom, Store,
};

/// The children of the node a path `X` from the root `R` leads to.
#[cfg(test)]
pub(crate) type ParentChildren<R, X> = ChildrenOf<<X as PathFrom<R>>::Target>;

/// The parent keeps a child of this kind under the key already.
#[derive(Debug, Snafu, PartialEq, Eq, Clone, Copy)]
#[snafu(display("the parent has a child of this kind under this key"))]
pub struct OccupiedError;

/// No node is at the parent's path.
#[cfg(test)]
#[derive(Debug, Snafu, PartialEq, Eq, Clone, Copy)]
#[snafu(display("no node at the parent's path"))]
pub(crate) struct NoParentError;

/// Why a node could not be added; the node comes back.
#[cfg(test)]
#[derive(Debug, Snafu)]
#[snafu(module)]
pub(crate) enum InsertError<N> {
    /// No node at the parent's path.
    #[snafu(display("the node was not added"))]
    NoParent {
        /// The node that was not added.
        node: N,
        /// Why.
        source: NoParentError,
    },
    /// The parent has a child of this kind under the same key.
    #[snafu(display("the node was not added"))]
    Occupied {
        /// The node that was not added.
        node: N,
        /// Why.
        source: OccupiedError,
    },
}
/// Add `node` as a child of the node at `at`, under `key`; the child's
/// position among the parent's children is inferred from its kind.
/// Returns the child's path.
#[allow(clippy::type_complexity)]
#[cfg(test)]
pub(crate) fn insert<R, P, X, CK, I>(
    root: &mut Node<R, P>,
    at: X,
    key: KeyAt<ParentChildren<R, X>, I>,
    node: Node<CK, P>,
) -> Result<<X as Append<I, HandleAt<ParentChildren<R, X>, I>>>::Out, InsertError<Node<CK, P>>>
where
    R: KindFor<P>,
    CK: KindFor<P>,
    X: NodePath<R, P> + Append<I, HandleAt<ParentChildren<R, X>, I>>,
    ParentChildren<R, X>: ChildAt<I, P> + ChildOfKind<CK, I>,
{
    let Some(parent) = at.node_mut(root) else {
        return Err(InsertError::NoParent {
            node,
            source: NoParentError,
        });
    };
    let store = <ParentChildren<R, X> as ChildAt<I, P>>::store_mut(&mut parent.children);
    match store.insert(key, node) {
        Ok(h) => Ok(at.append(h)),
        Err(Occupied(node)) => Err(InsertError::Occupied {
            node,
            source: OccupiedError,
        }),
    }
}

/// Why a node could not be moved; it stays where it was.
#[derive(Debug, Snafu, PartialEq, Eq)]
#[snafu(module)]
pub enum MoveError {
    /// No node at the path to move.
    #[snafu(display("no node at the path to move"))]
    NoSource,
    /// No node at the new parent's path.
    #[snafu(display("no node at the new parent's path"))]
    NoDestination,
    /// The new parent has a child of this kind under the same key.
    #[snafu(display("the node was not moved"))]
    Occupied {
        /// Why.
        source: OccupiedError,
    },
}

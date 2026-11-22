//! Adding, removing and moving nodes.
use core::fmt;

use super::{
    Append, ChildAt, ChildOfKind, ChildStorage, ChildrenOf, HandleAt, KeyAt, KindAt, KindFor, Node,
    NodePath, Occupied, PathFrom, SplitLast, Store,
};

/// The children of the node a path `X` from the root `R` leads to.
pub type ParentChildren<R, X> = ChildrenOf<<X as PathFrom<R>>::Target>;

/// Why a node could not be added; the node comes back.
#[derive(Debug)]
pub enum InsertError<N> {
    /// No node at the parent's path.
    NoParent(N),
    /// The parent has a child of this kind under the same key.
    Occupied(N),
}
impl<N> InsertError<N> {
    /// The node that was not added.
    pub fn into_node(self) -> N {
        match self {
            Self::NoParent(n) | Self::Occupied(n) => n,
        }
    }
}
impl<N> fmt::Display for InsertError<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoParent(_) => "no node at the parent's path",
            Self::Occupied(_) => "the parent has a child of this kind under this key",
        })
    }
}
impl<N: fmt::Debug> core::error::Error for InsertError<N> {}

/// Add `node` as a child of the node at `at`, under `key`; the child's
/// position among the parent's children is inferred from its kind.
/// Returns the child's path.
#[allow(clippy::type_complexity)]
pub fn insert<R, P, X, CK, I>(
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
        return Err(InsertError::NoParent(node));
    };
    let store = <ParentChildren<R, X> as ChildAt<I, P>>::store_mut(&mut parent.children);
    match store.insert(key, node) {
        Ok(h) => Ok(at.append(h)),
        Err(Occupied(n)) => Err(InsertError::Occupied(n)),
    }
}

/// The children of the parent of the node a path `X` from `R` leads to.
pub type LastChildren<R, X> = ChildrenOf<<<X as SplitLast>::Parent as PathFrom<R>>::Target>;
/// The kind of the node a path `X` from `R` leads to, through its parent.
pub type LastKind<R, X> = KindAt<LastChildren<R, X>, <X as SplitLast>::Index>;

/// Take the node at `at` out of the tree, with every node below it.
///
/// The nodes below it go with it, so they are removed before it; a node
/// that others link to is removed only after them (see
/// [`removal_order`](super::removal_order)), which is the caller's to keep.
pub fn remove<R, P, X>(root: &mut Node<R, P>, at: X) -> Option<Node<LastKind<R, X>, P>>
where
    R: KindFor<P>,
    X: SplitLast,
    X::Parent: NodePath<R, P>,
    LastChildren<R, X>: ChildAt<X::Index, P, Storage: ChildStorage<Handle = X::Handle>>,
{
    let (parent, h) = at.split_last();
    let parent = parent.node_mut(root)?;
    <LastChildren<R, X> as ChildAt<X::Index, P>>::store_mut(&mut parent.children).remove(h)
}

/// Why a node could not be moved; it stays where it was.
#[derive(Debug, PartialEq, Eq)]
pub enum MoveError {
    /// No node at the path to move.
    NoSource,
    /// No node at the new parent's path.
    NoDestination,
    /// The new parent has a child of this kind under the same key.
    Occupied,
}
impl fmt::Display for MoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoSource => "no node at the path to move",
            Self::NoDestination => "no node at the new parent's path",
            Self::Occupied => "the new parent has a child of this kind under this key",
        })
    }
}
impl core::error::Error for MoveError {}

/// Move the node at `at`, with every node below it, under `to`, a node of
/// the kind of its parent, keeping its key. Returns the new path.
///
/// When the move fails the node goes back under its parent; a store that
/// reuses slots may give it another handle there.
#[allow(clippy::type_complexity)]
pub fn move_to<R, P, X, B>(
    root: &mut Node<R, P>,
    at: X,
    to: B,
) -> Result<<B as Append<X::Index, X::Handle>>::Out, MoveError>
where
    R: KindFor<P>,
    X: SplitLast,
    X::Parent: NodePath<R, P>,
    B: NodePath<R, P, Target = <X::Parent as PathFrom<R>>::Target> + Append<X::Index, X::Handle>,
    LastChildren<R, X>: ChildAt<X::Index, P, Storage: ChildStorage<Handle = X::Handle>>,
{
    let (parent, h) = at.split_last();
    let from = parent.node_mut(root).ok_or(MoveError::NoSource)?;
    let store = <LastChildren<R, X> as ChildAt<X::Index, P>>::store_mut(&mut from.children);
    let key = store.key(h).ok_or(MoveError::NoSource)?.clone();
    let node = store.remove(h).ok_or(MoveError::NoSource)?;
    let put_back = |root: &mut Node<R, P>, key, node| {
        // Invariant: the source parent was reached above and nothing has
        // removed it since; its key was just freed, so the insert succeeds.
        let from = parent
            .node_mut(root)
            .expect("the source parent was reached a moment ago");
        let store = <LastChildren<R, X> as ChildAt<X::Index, P>>::store_mut(&mut from.children);
        let _ = store.insert(key, node);
    };
    let Some(dest) = to.node_mut(root) else {
        put_back(root, key, node);
        return Err(MoveError::NoDestination);
    };
    let store = <LastChildren<R, X> as ChildAt<X::Index, P>>::store_mut(&mut dest.children);
    match store.insert(key.clone(), node) {
        Ok(h) => Ok(to.append(h)),
        Err(Occupied(node)) => {
            put_back(root, key, node);
            Err(MoveError::Occupied)
        }
    }
}

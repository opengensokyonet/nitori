//! Putting a node into the tree and taking it out by its full key, along
//! the route of its kind.
use frunk_core::hlist::{HCons, HNil};

use crate::{
    lend::tree::SplitKey,
    structure::{ChildAt, ChildIndex, KindAt, KindFor, Node, NodeIdent, Store},
};

/// The place of a node down the route `Rt` from a node of the kind `K`
/// (`Self` is `(K, Rt)`), by what is left `F` of its full key, for the
/// parts `P`.
pub trait FkPlace<P, F> {
    /// The kind the route starts at.
    type Kind0: KindFor<P>;
    /// The kind the route leads to.
    type Target: KindFor<P>;
    /// Whether the place is free: `None` when its parent is not there, or
    /// the route is empty.
    fn vacant(n: &Node<Self::Kind0, P>, full: &F) -> Option<bool>;
    /// Put `node` at its place, giving which node its parent is; it comes
    /// back when its parent is not there or keeps another node under its
    /// key, or the route is empty.
    #[allow(clippy::result_large_err)]
    fn insert(
        n: &mut Node<Self::Kind0, P>,
        full: &F,
        node: Node<Self::Target, P>,
    ) -> Result<NodeIdent, Node<Self::Target, P>>;
    /// Take the node at its place out, with every node below it; `None`
    /// when it is not there, or the route is empty.
    fn remove(n: &mut Node<Self::Kind0, P>, full: &F) -> Option<Node<Self::Target, P>>;
}
impl<K: KindFor<P>, P> FkPlace<P, HNil> for (K, HNil) {
    type Kind0 = K;
    type Target = K;
    fn vacant(_: &Node<K, P>, _: &HNil) -> Option<bool> {
        None
    }
    fn insert(_: &mut Node<K, P>, _: &HNil, node: Node<K, P>) -> Result<NodeIdent, Node<K, P>> {
        Err(node)
    }
    fn remove(_: &mut Node<K, P>, _: &HNil) -> Option<Node<K, P>> {
        None
    }
}
impl<K: KindFor<P>, P, F, J> FkPlace<P, F> for (K, HCons<J, HNil>)
where
    K::Children: ChildAt<J, P>,
    <K::Children as ChildIndex<J>>::Storage: SplitKey<F>,
{
    type Kind0 = K;
    type Target = KindAt<K::Children, J>;
    fn vacant(n: &Node<K, P>, full: &F) -> Option<bool> {
        let (key, _) = <<K::Children as ChildIndex<J>>::Storage as SplitKey<F>>::split_key(full);
        Some(
            <K::Children as ChildAt<J, P>>::store(&n.children)
                .find(&key)
                .is_none(),
        )
    }
    fn insert(
        n: &mut Node<K, P>,
        full: &F,
        node: Node<Self::Target, P>,
    ) -> Result<NodeIdent, Node<Self::Target, P>> {
        let (key, _) = <<K::Children as ChildIndex<J>>::Storage as SplitKey<F>>::split_key(full);
        <K::Children as ChildAt<J, P>>::store_mut(&mut n.children)
            .insert(key, node)
            .map(|_| n.ident)
            .map_err(|o| o.0)
    }
    fn remove(n: &mut Node<K, P>, full: &F) -> Option<Node<Self::Target, P>> {
        let (key, _) = <<K::Children as ChildIndex<J>>::Storage as SplitKey<F>>::split_key(full);
        let store = <K::Children as ChildAt<J, P>>::store_mut(&mut n.children);
        let h = store.find(&key)?;
        store.remove(h)
    }
}
type Down<K, J> = KindAt<<K as crate::structure::NodeKind>::Children, J>;
type Rest<K, J, F> =
    <<<K as crate::structure::NodeKind>::Children as ChildIndex<J>>::Storage as SplitKey<F>>::Rest;
impl<K: KindFor<P>, P, F, J, J2, R> FkPlace<P, F> for (K, HCons<J, HCons<J2, R>>)
where
    K::Children: ChildAt<J, P>,
    <K::Children as ChildIndex<J>>::Storage: SplitKey<F>,
    (Down<K, J>, HCons<J2, R>): FkPlace<P, Rest<K, J, F>, Kind0 = Down<K, J>>,
{
    type Kind0 = K;
    type Target = <(Down<K, J>, HCons<J2, R>) as FkPlace<P, Rest<K, J, F>>>::Target;
    fn vacant(n: &Node<K, P>, full: &F) -> Option<bool> {
        let (key, rest) = <<K::Children as ChildIndex<J>>::Storage as SplitKey<F>>::split_key(full);
        let store = <K::Children as ChildAt<J, P>>::store(&n.children);
        let child = store.find(&key).and_then(|h| store.get(h))?;
        <(Down<K, J>, HCons<J2, R>) as FkPlace<P, _>>::vacant(child, rest)
    }
    fn insert(
        n: &mut Node<K, P>,
        full: &F,
        node: Node<Self::Target, P>,
    ) -> Result<NodeIdent, Node<Self::Target, P>> {
        let (key, rest) = <<K::Children as ChildIndex<J>>::Storage as SplitKey<F>>::split_key(full);
        let store = <K::Children as ChildAt<J, P>>::store_mut(&mut n.children);
        let Some(child) = store.find(&key).and_then(|h| store.get_mut(h)) else {
            return Err(node);
        };
        <(Down<K, J>, HCons<J2, R>) as FkPlace<P, _>>::insert(child, rest, node)
    }
    fn remove(n: &mut Node<K, P>, full: &F) -> Option<Node<Self::Target, P>> {
        let (key, rest) = <<K::Children as ChildIndex<J>>::Storage as SplitKey<F>>::split_key(full);
        let store = <K::Children as ChildAt<J, P>>::store_mut(&mut n.children);
        let child = store.find(&key).and_then(|h| store.get_mut(h))?;
        <(Down<K, J>, HCons<J2, R>) as FkPlace<P, _>>::remove(child, rest)
    }
}

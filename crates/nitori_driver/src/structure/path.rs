//! Typed paths: from a node down to one below it, a step per level.
use core::marker::PhantomData;

use super::{
    Address, ChildAt, ChildIndex, ChildStorage, KeyAt, KindAt, KindFor, Node, NodeKind, Store,
};

/// The typed path to the node itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct End;

/// A typed path one level down: the child at the position `I` of the list
/// of children, at the handle `handle`, then `rest` below it.
pub struct Step<I, H, R> {
    /// The child's handle in its store.
    pub handle: H,
    /// The path below the child.
    pub rest: R,
    _index: PhantomData<fn() -> I>,
}
impl<I, H, R> Step<I, H, R> {
    /// The path to the child at `handle`, then `rest`.
    pub fn new(handle: H, rest: R) -> Self {
        Self {
            handle,
            rest,
            _index: PhantomData,
        }
    }
}
impl<I, H: Copy, R: Copy> Clone for Step<I, H, R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<I, H: Copy, R: Copy> Copy for Step<I, H, R> {}
impl<I, H: PartialEq, R: PartialEq> PartialEq for Step<I, H, R> {
    fn eq(&self, other: &Self) -> bool {
        self.handle == other.handle && self.rest == other.rest
    }
}
impl<I, H: Eq, R: Eq> Eq for Step<I, H, R> {}
impl<I, H: core::fmt::Debug, R: core::fmt::Debug> core::fmt::Debug for Step<I, H, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}/{:?}", self.handle, self.rest)
    }
}

/// A typed path from a node of the kind `K`; it follows from the kinds
/// alone.
pub trait PathFrom<K: NodeKind>: Copy {
    /// The kind of the node the path leads to.
    type Target: NodeKind;
    /// The driver's address of the same node.
    fn address(&self) -> Address<K>;
}
impl<K: NodeKind> PathFrom<K> for End {
    type Target = K;
    fn address(&self) -> Address<K> {
        Address::here()
    }
}
impl<K: NodeKind, I, H, R> PathFrom<K> for Step<I, H, R>
where
    K::Children: ChildIndex<I, Storage: ChildStorage<Handle = H>>,
    R: PathFrom<KindAt<K::Children, I>>,
    H: Copy,
{
    type Target = R::Target;
    fn address(&self) -> Address<K> {
        Address::below(<K::Children as ChildIndex<I>>::inject(
            self.handle,
            self.rest.address(),
        ))
    }
}

/// Reaching the node a path leads to.
pub trait NodePath<K: KindFor<P>, P>: PathFrom<K, Target: KindFor<P>> {
    /// The node, if it is there.
    fn node<'a>(&self, n: &'a Node<K, P>) -> Option<&'a Node<Self::Target, P>>;
    /// The node, exclusive, if it is there.
    fn node_mut<'a>(&self, n: &'a mut Node<K, P>) -> Option<&'a mut Node<Self::Target, P>>;
}
impl<K: KindFor<P>, P> NodePath<K, P> for End {
    fn node<'a>(&self, n: &'a Node<K, P>) -> Option<&'a Node<K, P>> {
        Some(n)
    }
    fn node_mut<'a>(&self, n: &'a mut Node<K, P>) -> Option<&'a mut Node<K, P>> {
        Some(n)
    }
}
impl<K: KindFor<P>, P, I, H: Copy, R> NodePath<K, P> for Step<I, H, R>
where
    K::Children: ChildAt<I, P, Storage: ChildStorage<Handle = H>>,
    R: NodePath<KindAt<K::Children, I>, P>,
{
    fn node<'a>(&self, n: &'a Node<K, P>) -> Option<&'a Node<R::Target, P>> {
        let store = <K::Children as ChildAt<I, P>>::store(&n.children);
        self.rest.node(store.get(self.handle)?)
    }
    fn node_mut<'a>(&self, n: &'a mut Node<K, P>) -> Option<&'a mut Node<R::Target, P>> {
        let store = <K::Children as ChildAt<I, P>>::store_mut(&mut n.children);
        self.rest.node_mut(store.get_mut(self.handle)?)
    }
}

/// The key the node a path leads to has under its parent. For the path
/// [`End`] the caller gives it (`D`).
pub trait KeyedPath<K: KindFor<P>, P, D>: NodePath<K, P> {
    /// The key's type.
    type Key;
    /// The key, if the node is there.
    fn key_of<'a>(&self, n: &'a Node<K, P>, d: &'a D) -> Option<&'a Self::Key>;
}
impl<K: KindFor<P>, P, D> KeyedPath<K, P, D> for End {
    type Key = D;
    fn key_of<'a>(&self, _: &'a Node<K, P>, d: &'a D) -> Option<&'a D> {
        Some(d)
    }
}
impl<K: KindFor<P>, P, D, I, H: Copy, R> KeyedPath<K, P, D> for Step<I, H, R>
where
    K::Children: ChildAt<I, P, Storage: ChildStorage<Handle = H>>,
    R: KeyedPath<KindAt<K::Children, I>, P, KeyAt<K::Children, I>>,
{
    type Key = R::Key;
    fn key_of<'a>(&self, n: &'a Node<K, P>, _: &'a D) -> Option<&'a R::Key> {
        let store = <K::Children as ChildAt<I, P>>::store(&n.children);
        self.rest
            .key_of(store.get(self.handle)?, store.key(self.handle)?)
    }
}

/// The path one step further: to the child at `I`, at a handle `H`.
pub trait Append<I, H> {
    /// The longer path.
    type Out;
    /// Add the step.
    fn append(self, handle: H) -> Self::Out;
}
impl<I, H> Append<I, H> for End {
    type Out = Step<I, H, End>;
    fn append(self, handle: H) -> Self::Out {
        Step::new(handle, End)
    }
}
impl<I, H, J, G, R: Append<I, H>> Append<I, H> for Step<J, G, R> {
    type Out = Step<J, G, R::Out>;
    fn append(self, handle: H) -> Self::Out {
        Step::new(self.handle, self.rest.append(handle))
    }
}

/// A path that is at least one step long, split into its parent's path
/// and its last step.
pub trait SplitLast {
    /// The parent's path.
    type Parent;
    /// The position of the last step's child.
    type Index;
    /// The last step's handle.
    type Handle: Copy;
    /// Split the path.
    fn split_last(self) -> (Self::Parent, Self::Handle);
}
impl<I, H: Copy> SplitLast for Step<I, H, End> {
    type Parent = End;
    type Index = I;
    type Handle = H;
    fn split_last(self) -> (End, H) {
        (End, self.handle)
    }
}
impl<I, H: Copy, J, G, R> SplitLast for Step<I, H, Step<J, G, R>>
where
    Step<J, G, R>: SplitLast,
{
    type Parent = Step<I, H, <Step<J, G, R> as SplitLast>::Parent>;
    type Index = <Step<J, G, R> as SplitLast>::Index;
    type Handle = <Step<J, G, R> as SplitLast>::Handle;
    fn split_last(self) -> (Self::Parent, Self::Handle) {
        let (parent, h) = self.rest.split_last();
        (Step::new(self.handle, parent), h)
    }
}

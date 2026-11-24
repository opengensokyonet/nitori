//! Nodes: the parts' shares and the stores of the children.
use core::{marker::PhantomData, ops::Add};

use frunk_core::{
    hlist::{HCons, HList, HNil},
    indices::{Here, There},
    traits::{ToMut, ToRef},
};

use super::{Child, ChildIndex, ChildList, ChildStorage, NodeKind, StoreOf};

/// The share the part `Self` keeps in each node of the kind `K`: a list of
/// components (`HNil` for none).
pub trait Share<K> {
    /// The components.
    type Data: HList + 'static;
    /// What the part keeps of its share after the root of the tree ended;
    /// `()` by default. Only the root's is kept.
    type Remains: Default + 'static = ();
    /// The reactions of the part at the nodes of this kind: a list of
    /// [`ReactsTo`](crate::react::ReactsTo)`<point, reaction>`, none by
    /// default.
    type Reacts: 'static = HNil;
    /// The kinds of the calls the part keeps at each node of this kind, an
    /// `HList`; none by default. A new node's calls start after its
    /// components are built, in the order of the parts, then in this order
    /// ([`CallKind::start`](crate::call::CallKind::start)).
    type Calls: HList + 'static = HNil;
}

/// What the parts `Self` keep of their shares in a node of the kind `K`
/// after it ended, one per part, in the order of the parts.
pub trait RemainsList<K>: 'static {
    /// The list.
    type Remains: Default + HList + 'static;
}
impl<K> RemainsList<K> for HNil {
    type Remains = HNil;
}
impl<K, Pt: Share<K> + 'static, T: RemainsList<K>> RemainsList<K> for HCons<Pt, T> {
    type Remains = HCons<Pt::Remains, T::Remains>;
}
/// What the parts `P` keep after a node of the kind `K` ended.
pub type RemainsOf<P, K> = <P as RemainsList<K>>::Remains;

/// The shares of a list of parts in a node of the kind `K`, one list per
/// part, in the order of the parts.
pub trait Shares<K>: 'static {
    /// The list of the shares.
    type Data: 'static;
}
impl<K> Shares<K> for HNil {
    type Data = HNil;
}
impl<K, Pt: Share<K> + 'static, T: Shares<K>> Shares<K> for HCons<Pt, T> {
    type Data = HCons<Pt::Data, T::Data>;
}

/// A kind whose nodes are built for the list of parts `P`: every part has
/// a share in it and in every kind below it.
pub trait KindFor<P>: NodeKind<Children: ChildStores<P>> {
    /// The parts' shares in a node of this kind.
    type Data: 'static;
}
impl<K, P> KindFor<P> for K
where
    K: NodeKind<Children: ChildStores<P>>,
    P: Shares<K>,
{
    type Data = <P as Shares<K>>::Data;
}

/// The stores of a list of children for the parts `P`: each storage kind's
/// store of the child kind's nodes.
pub trait ChildStores<P>: ChildList {
    /// One store per child, in the order of the list.
    type Stores: Default + HList;
}
impl<P> ChildStores<P> for HNil {
    type Stores = HNil;
}
impl<P, CK: KindFor<P>, S: ChildStorage, T: ChildStores<P>> ChildStores<P>
    for HCons<Child<CK, S>, T>
{
    type Stores = HCons<S::Store<Node<CK, P>>, T::Stores>;
}

/// A node of the kind `K` for the parts `P`: the parts' shares and the
/// stores of its children.
pub struct Node<K: KindFor<P>, P> {
    /// The parts' shares, one list per part.
    pub data: K::Data,
    /// The stores of the children, one per child of the kind.
    pub children: <K::Children as ChildStores<P>>::Stores,
    _parts: PhantomData<fn() -> P>,
}
impl<K: KindFor<P>, P> Node<K, P> {
    /// A node with the shares `data` and no children.
    pub fn new(data: K::Data) -> Self {
        Self {
            data,
            children: Default::default(),
            _parts: PhantomData,
        }
    }
}

/// The store of the child at `I`, for the parts `P`.
pub trait ChildAt<I, P>: ChildIndex<I, Kind: KindFor<P>> + ChildStores<P> {
    /// The store.
    fn store(stores: &Self::Stores) -> &StoreOf<Self::Storage, Node<Self::Kind, P>>;
    /// The store, exclusive.
    fn store_mut(stores: &mut Self::Stores) -> &mut StoreOf<Self::Storage, Node<Self::Kind, P>>;
}
impl<P, CK: KindFor<P>, S: ChildStorage, T: ChildStores<P>> ChildAt<Here, P>
    for HCons<Child<CK, S>, T>
{
    fn store(stores: &Self::Stores) -> &S::Store<Node<CK, P>> {
        &stores.head
    }
    fn store_mut(stores: &mut Self::Stores) -> &mut S::Store<Node<CK, P>> {
        &mut stores.head
    }
}
impl<P, I, CK: KindFor<P>, S: ChildStorage, T: ChildAt<I, P>> ChildAt<There<I>, P>
    for HCons<Child<CK, S>, T>
{
    fn store(stores: &Self::Stores) -> &StoreOf<T::Storage, Node<T::Kind, P>> {
        T::store(&stores.tail)
    }
    fn store_mut(stores: &mut Self::Stores) -> &mut StoreOf<T::Storage, Node<T::Kind, P>> {
        T::store_mut(&mut stores.tail)
    }
}

/// A node's data, a list of the parts' lists, borrowed exclusive as one
/// flat list of components.
pub trait FlatMut<'a> {
    /// The flat list of `&mut` components.
    type Out;
    /// Borrow every component.
    fn flat_mut(&'a mut self) -> Self::Out;
}
impl<'a> FlatMut<'a> for HNil {
    type Out = HNil;
    fn flat_mut(&'a mut self) -> HNil {
        HNil
    }
}
impl<'a, S: ToMut<'a> + 'a, Ss: FlatMut<'a> + 'a> FlatMut<'a> for HCons<S, Ss>
where
    S::Output: Add<Ss::Out>,
{
    type Out = <S::Output as Add<Ss::Out>>::Output;
    fn flat_mut(&'a mut self) -> Self::Out {
        let HCons { head, tail } = self;
        head.to_mut() + tail.flat_mut()
    }
}

/// A node's data borrowed shared as one flat list of components.
pub trait FlatRef<'a> {
    /// The flat list of `&` components.
    type Out;
    /// Borrow every component.
    fn flat_ref(&'a self) -> Self::Out;
}
impl<'a> FlatRef<'a> for HNil {
    type Out = HNil;
    fn flat_ref(&'a self) -> HNil {
        HNil
    }
}
impl<'a, S: ToRef<'a> + 'a, Ss: FlatRef<'a> + 'a> FlatRef<'a> for HCons<S, Ss>
where
    S::Output: Add<Ss::Out>,
{
    type Out = <S::Output as Add<Ss::Out>>::Output;
    fn flat_ref(&'a self) -> Self::Out {
        self.head.to_ref() + self.tail.flat_ref()
    }
}

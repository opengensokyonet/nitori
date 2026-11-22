//! Node kinds, their children and their links.
use alloc::vec::Vec;
use core::{any::TypeId, marker::PhantomData};

use frunk_core::{
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::{Address, ChildStorage};

/// A kind of node: the kinds of its children and the nodes it links to.
///
/// A kind names the same children wherever it is used; two structures
/// that need different children use different kinds.
pub trait NodeKind: 'static + Sized {
    /// The children, an `HList` of [`Child`]`<kind, storage kind>`.
    type Children: ChildList;
    /// The nodes a node of this kind links to, an `HList` of [`Link`]s
    /// (`HNil` for none).
    type Linked: LinkList<Self>;
}

/// A child in a [`NodeKind::Children`] list: the kind `K`, kept by the
/// storage kind `S`.
pub struct Child<K, S>(PhantomData<fn() -> (K, S)>);

/// A list of children; the addresses below a node follow from it alone.
pub trait ChildList: 'static {
    /// Which child and its handle, then the address below it: a sum with
    /// one alternative for each child.
    type Addr: Copy + Eq + core::fmt::Debug;
    /// The kinds of the children and of every node below them that are the
    /// target of a link, pushed onto `out`.
    fn link_targets(out: &mut Vec<TypeId>);
}
impl ChildList for HNil {
    type Addr = CNil;
    fn link_targets(_: &mut Vec<TypeId>) {}
}
impl<K: NodeKind, S: ChildStorage, T: ChildList> ChildList for HCons<Child<K, S>, T> {
    type Addr = Coproduct<(S::Handle, Address<K>), T::Addr>;
    fn link_targets(out: &mut Vec<TypeId>) {
        K::Linked::targets(out);
        K::Children::link_targets(out);
        T::link_targets(out);
    }
}

/// The child at the position `I` of a list.
pub trait ChildIndex<I>: ChildList {
    /// Its kind.
    type Kind: NodeKind;
    /// Its storage kind.
    type Storage: ChildStorage;
    /// The address, in the list's terms, of the node at `below` under the
    /// child at `handle`.
    fn inject(
        handle: <Self::Storage as ChildStorage>::Handle,
        below: Address<Self::Kind>,
    ) -> Self::Addr;
}
impl<K: NodeKind, S: ChildStorage, T: ChildList> ChildIndex<Here> for HCons<Child<K, S>, T> {
    type Kind = K;
    type Storage = S;
    fn inject(handle: S::Handle, below: Address<K>) -> Self::Addr {
        Coproduct::Inl((handle, below))
    }
}
impl<I, K: NodeKind, S: ChildStorage, T: ChildIndex<I>> ChildIndex<There<I>>
    for HCons<Child<K, S>, T>
{
    type Kind = T::Kind;
    type Storage = T::Storage;
    fn inject(handle: <T::Storage as ChildStorage>::Handle, below: Address<T::Kind>) -> Self::Addr {
        Coproduct::Inr(T::inject(handle, below))
    }
}

/// The child of the kind `CK` is at the position `I`; `I` is inferred, as
/// for frunk's `Selector`.
pub trait ChildOfKind<CK, I>: ChildIndex<I, Kind = CK> {}
impl<CK: NodeKind, S: ChildStorage, T: ChildList> ChildOfKind<CK, Here> for HCons<Child<CK, S>, T> {}
impl<CK, I, HK: NodeKind, HS: ChildStorage, T: ChildOfKind<CK, I>> ChildOfKind<CK, There<I>>
    for HCons<Child<HK, HS>, T>
{
}

/// The children of the kind `K`.
pub type ChildrenOf<K> = <K as NodeKind>::Children;
/// The kind of the child at `I` of the list `L`.
pub type KindAt<L, I> = <L as ChildIndex<I>>::Kind;
/// The storage kind of the child at `I` of the list `L`.
pub type StorageAt<L, I> = <L as ChildIndex<I>>::Storage;
/// The key of the child at `I` of the list `L`.
pub type KeyAt<L, I> = <StorageAt<L, I> as ChildStorage>::Key;
/// The handle of the child at `I` of the list `L`.
pub type HandleAt<L, I> = <StorageAt<L, I> as ChildStorage>::Handle;

/// The links of a node of the kind `K`.
pub trait LinkList<K: NodeKind> {
    /// The kinds the links point to, pushed onto `out`.
    fn targets(out: &mut Vec<TypeId>);
}
impl<K: NodeKind> LinkList<K> for HNil {
    fn targets(_: &mut Vec<TypeId>) {}
}
impl<K: NodeKind, L: Link<K>, T: LinkList<K>> LinkList<K> for HCons<L, T> {
    fn targets(out: &mut Vec<TypeId>) {
        out.push(TypeId::of::<L::Target>());
        T::targets(out);
    }
}

/// A link of a node of the kind `K` to a node that is not above it.
///
/// The linked node is there as long as the node linking to it is: the
/// structure starts and ends them together, and a node is removed only
/// after the last node linking to it ([`removal_order`](super::removal_order)).
pub trait Link<K: NodeKind> {
    /// The kind of the linked node.
    type Target: NodeKind;
}

/// A link to the root's child of the kind `CK` kept under the linking
/// node's own key, the key it has under its own parent. Its position among
/// the root's children is inferred where the root is known.
pub struct ByOwnKey<CK>(PhantomData<fn() -> CK>);
impl<K: NodeKind, CK: NodeKind> Link<K> for ByOwnKey<CK> {
    type Target = CK;
}

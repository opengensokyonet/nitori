//! Node kinds, their children and their links.
use alloc::vec::Vec;
use core::{any::TypeId, convert::Infallible, marker::PhantomData};

use frunk_core::{
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::{Address, ChildStorage, KeyRoute};
use crate::request::Request;

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
    /// Why a node of this kind is asked to end, as its components' end
    /// hooks are told ([`Hooks::end`](crate::hooks::Hooks::end),
    /// [`EndCause::Reason`](crate::hooks::EndCause::Reason)); `()` by
    /// default. A node that ends because a node above it ends, because no
    /// node links to it any more or because the driver was dropped is told
    /// so instead.
    type EndReason: 'static = ();
    /// What the one adding a node of this kind gives it to start from,
    /// handed to every component's [`build`](crate::hooks::Hooks::build)
    /// in turn; `()` by default. A component takes out what it keeps of
    /// it, such as a value that moves once.
    type Start: 'static = ();
    /// What a node of this kind is asked to change in its own subtree, and
    /// how it does it ([`Request`]); `Infallible` for a kind that is asked
    /// nothing.
    type Request: Request<Self> = Infallible;
}

/// The kind and the storage kind of the first child of an empty list of
/// children: there is none, and neither has a value.
pub enum NoChild {}
impl NodeKind for NoChild {
    type Children = HNil;
    type Linked = HNil;
    type Start = Infallible;
}
impl super::storage::sealed::Sealed for NoChild {}
impl ChildStorage for NoChild {
    type Key = Infallible;
    type Handle = Infallible;
    type Store<N> = NoStore;
}
/// The store of [`NoChild`]: it keeps nothing.
#[derive(Default)]
pub struct NoStore;
impl<N> super::Store<N, Infallible, Infallible> for NoStore {
    fn insert(&mut self, key: Infallible, _: N) -> Result<Infallible, super::Occupied<N>> {
        match key {}
    }
    fn remove(&mut self, at: Infallible) -> Option<N> {
        match at {}
    }
    fn get(&self, at: Infallible) -> Option<&N> {
        match at {}
    }
    fn get_mut(&mut self, at: Infallible) -> Option<&mut N> {
        match at {}
    }
    fn key(&self, at: Infallible) -> Option<&Infallible> {
        match at {}
    }
    fn find(&self, key: &Infallible) -> Option<Infallible> {
        match *key {}
    }
    fn handles(&self) -> impl Iterator<Item = Infallible> {
        core::iter::empty()
    }
}

/// A child in a [`NodeKind::Children`] list: the kind `K`, kept by the
/// storage kind `S`.
pub struct Child<K, S>(PhantomData<fn() -> (K, S)>);

/// A list of children; the addresses below a node follow from it alone.
pub trait ChildList: 'static {
    /// Which child and its handle, then the address below it: a sum with
    /// one alternative for each child.
    type Addr: Copy + Eq + core::fmt::Debug;
    /// Which child and its key, then the key address below it: a sum with
    /// one alternative for each child.
    type KeyAddr: Clone + PartialEq + 'static;
    /// The kinds of the children and of every node below them that are the
    /// target of a link, pushed onto `out`.
    fn link_targets(out: &mut Vec<TypeId>);
    /// `addr` with its first `levels` levels (at least one) taken from
    /// `prefix`; `None` when the two go to different children there.
    fn splice(addr: &Self::Addr, prefix: &Self::Addr, levels: usize) -> Option<Self::Addr>;
    /// The kind of the first child ([`NoChild`] for none).
    type HeadKind: NodeKind;
    /// The storage kind of the first child ([`NoChild`] for none).
    type HeadStorage: ChildStorage;
    /// The other children.
    type Tail: ChildList;
}
impl ChildList for HNil {
    type HeadKind = NoChild;
    type HeadStorage = NoChild;
    type Tail = HNil;
    type Addr = CNil;
    type KeyAddr = CNil;
    fn link_targets(_: &mut Vec<TypeId>) {}
    fn splice(addr: &CNil, _: &CNil, _: usize) -> Option<CNil> {
        match *addr {}
    }
}
impl<K: NodeKind, S: ChildStorage, T: ChildList> ChildList for HCons<Child<K, S>, T> {
    type HeadKind = K;
    type HeadStorage = S;
    type Tail = T;
    type Addr = Coproduct<(S::Handle, Address<K>), T::Addr>;
    type KeyAddr = Coproduct<(S::Key, KeyRoute<K>), T::KeyAddr>;
    fn link_targets(out: &mut Vec<TypeId>) {
        K::Linked::targets(out);
        K::Children::link_targets(out);
        T::link_targets(out);
    }
    fn splice(addr: &Self::Addr, prefix: &Self::Addr, levels: usize) -> Option<Self::Addr> {
        match (addr, prefix) {
            (Coproduct::Inl((_, below)), Coproduct::Inl((h, from))) => Some(Coproduct::Inl((
                *h,
                if levels <= 1 {
                    *below
                } else {
                    below.splice(from, levels - 1)?
                },
            ))),
            (Coproduct::Inr(a), Coproduct::Inr(b)) => T::splice(a, b, levels).map(Coproduct::Inr),
            _ => None,
        }
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
    /// The handle and the address below it, when `addr` is under the child
    /// at this position.
    #[allow(clippy::type_complexity)]
    fn project(
        addr: &Self::Addr,
    ) -> Option<(<Self::Storage as ChildStorage>::Handle, Address<Self::Kind>)>;
    /// The key address, in the list's terms, of the node at `below` under
    /// the child kept under `key`.
    fn inject_key(
        key: <Self::Storage as ChildStorage>::Key,
        below: KeyRoute<Self::Kind>,
    ) -> Self::KeyAddr;
}
impl<K: NodeKind, S: ChildStorage, T: ChildList> ChildIndex<Here> for HCons<Child<K, S>, T> {
    type Kind = K;
    type Storage = S;
    fn inject(handle: S::Handle, below: Address<K>) -> Self::Addr {
        Coproduct::Inl((handle, below))
    }
    fn project(addr: &Self::Addr) -> Option<(S::Handle, Address<K>)> {
        match addr {
            Coproduct::Inl(at) => Some(*at),
            Coproduct::Inr(_) => None,
        }
    }
    fn inject_key(key: S::Key, below: KeyRoute<K>) -> Self::KeyAddr {
        Coproduct::Inl((key, below))
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
    fn project(
        addr: &Self::Addr,
    ) -> Option<(<T::Storage as ChildStorage>::Handle, Address<T::Kind>)> {
        match addr {
            Coproduct::Inl(_) => None,
            Coproduct::Inr(rest) => T::project(rest),
        }
    }
    fn inject_key(
        key: <T::Storage as ChildStorage>::Key,
        below: KeyRoute<T::Kind>,
    ) -> Self::KeyAddr {
        Coproduct::Inr(T::inject_key(key, below))
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

/// The start a node of the kind `Self` gives the node of the kind `CK` it
/// links to, when that node is not there yet and is built first; not
/// called when it is there.
pub trait LinkedStart<CK: NodeKind>: NodeKind {
    /// The linked node's start, from the linking node's own; a part of it
    /// may be taken out.
    fn linked_start(start: &mut Self::Start) -> CK::Start;
}

/// A link of a node of the kind `K` to a node that is not above it.
///
/// The linked node is there as long as the node linking to it is: the
/// structure starts and ends them together, and a node is removed only
/// after the last node linking to it.
pub trait Link<K: NodeKind> {
    /// The kind of the linked node.
    type Target: NodeKind;
    /// The linked node's start, when the node linking to it builds it.
    fn linked_start(start: &mut K::Start) -> <Self::Target as NodeKind>::Start;
}

/// A link to the root's child of the kind `CK` kept under the linking
/// node's own key, the key it has under its own parent. Its position among
/// the root's children is inferred where the root is known; the linking
/// kind says what start the linked node gets ([`LinkedStart`]).
pub struct ByOwnKey<CK>(PhantomData<fn() -> CK>);
impl<K: LinkedStart<CK>, CK: NodeKind> Link<K> for ByOwnKey<CK> {
    type Target = CK;
    fn linked_start(start: &mut K::Start) -> CK::Start {
        K::linked_start(start)
    }
}

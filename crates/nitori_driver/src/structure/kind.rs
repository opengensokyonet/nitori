//! Node kinds and their children.
use core::marker::PhantomData;

use frunk_core::{
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::{Address, ChildStorage, KeyRoute};

/// A kind of node: the kinds of its children.
///
/// A kind names the same children wherever it is used; two structures
/// that need different children use different kinds.
pub trait NodeKind: 'static + Sized {
    /// The children, an `HList` of [`Child`]`<kind, storage kind>`.
    type Children: ChildList;
    /// Why a node of this kind is asked to end, when the one ending it
    /// gives a reason; `()` by default.
    type EndReason: 'static = ();
    /// What the one adding a node of this kind gives it to start from;
    /// `()` by default. Each component takes out what it keeps of it, such
    /// as a value that moves once, and the calls take what is left.
    type Start: 'static = ();
    /// What a node of this kind can be asked to change in its own subtree,
    /// one closed type; `!` by default, for nothing. A request names its
    /// node by key address ([`NodeRequest`](crate::request::NodeRequest)).
    type Request: 'static = !;
    /// What handles the requests of a node of this kind
    /// ([`HandleRequest`](crate::request::HandleRequest)); by default
    /// [`NoRequests`](crate::request::NoRequests), which handles only a
    /// `Request` of `!`. The composition checks that it handles them at
    /// its priority.
    type Handler: 'static = crate::request::NoRequests;
}

/// The kind and the storage kind of the first child of an empty list of
/// children: there is none, and neither has a value. Only a placeholder
/// for the empty list: a `Child<NoChild, NoChild>` written into a
/// structure can never be added, since its key and start have no value.
pub enum NoChild {}
impl NodeKind for NoChild {
    type Children = HNil;
    type Start = !;
}
impl super::storage::sealed::Sealed for NoChild {}
impl ChildStorage for NoChild {
    type Key = !;
    type Handle = !;
    type Store<N> = NoStore;
}
/// The store of [`NoChild`]: it keeps nothing.
#[derive(Default)]
pub struct NoStore;
impl<N> super::Store<N, !, !> for NoStore {
    fn insert(&mut self, key: !, _: N) -> Result<!, super::Occupied<N>> {
        match key {}
    }
    fn remove(&mut self, at: !) -> Option<N> {
        match at {}
    }
    fn get(&self, at: !) -> Option<&N> {
        match at {}
    }
    fn get_mut(&mut self, at: !) -> Option<&mut N> {
        match at {}
    }
    fn key(&self, at: !) -> Option<&!> {
        match at {}
    }
    fn find(&self, key: &!) -> Option<!> {
        match *key {}
    }
    fn handles(&self) -> impl Iterator<Item = !> {
        core::iter::empty()
    }
    fn iter_mut<'s>(&'s mut self) -> impl Iterator<Item = (&'s !, &'s mut N)>
    where
        N: 's,
    {
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

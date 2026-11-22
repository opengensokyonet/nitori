//! The driver's addresses: a string of handles from a node down, typed by
//! the kinds alone.
use alloc::{vec, vec::Vec};
use core::{any::TypeId, fmt};

use frunk_core::{
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil},
};

use super::{
    Child, ChildIndex, ChildList, ChildOfKind, ChildStorage, ChildStores, KeyAt, KindFor, LinkList,
    Node, NodeKind, Store,
};

/// The driver's address of a node at or below a node of the kind `K`.
///
/// It is the handles from that node down, one per level, and follows from
/// the kinds alone: its type names no part, and its size does not depend
/// on the parts. A typed path gives it ([`PathFrom::address`](super::PathFrom::address)).
pub struct Address<K: NodeKind>(Option<<K::Children as ChildList>::Addr>);
impl<K: NodeKind> Address<K> {
    /// The node itself.
    pub fn here() -> Self {
        Self(None)
    }
    /// A node below: which child and its handle, then the address below it.
    pub fn below(addr: <K::Children as ChildList>::Addr) -> Self {
        Self(Some(addr))
    }
    /// The address below the node, or `None` for the node itself.
    pub fn below_here(&self) -> Option<&<K::Children as ChildList>::Addr> {
        self.0.as_ref()
    }
    /// This address with its first `levels` levels taken from `prefix`: the
    /// address a node below a node that moved has afterwards. `None` when
    /// either is shorter, or they go to different children there.
    pub fn splice(&self, prefix: &Self, levels: usize) -> Option<Self> {
        if levels == 0 {
            return Some(*self);
        }
        match (&self.0, &prefix.0) {
            (Some(a), Some(b)) => K::Children::splice(a, b, levels).map(Self::below),
            _ => None,
        }
    }
}
impl<K: NodeKind> Clone for Address<K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K: NodeKind> Copy for Address<K> {}
impl<K: NodeKind> PartialEq for Address<K> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<K: NodeKind> Eq for Address<K> {}
impl<K: NodeKind> fmt::Debug for Address<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({:?})", self.0)
    }
}

/// The address of a node at or below a node of the kind `K` by the keys
/// its nodes are kept under, one per level: what a request names the node
/// it is for by ([`NodeRequest`](crate::request::NodeRequest)).
///
/// It follows from the kinds alone, as [`Address`] does; the driver finds
/// the node by it as the tree is now, so a node ended and built again
/// under the same key is the same address.
pub struct KeyAddress<K: NodeKind>(Option<<K::Children as ChildList>::KeyAddr>);
impl<K: NodeKind> KeyAddress<K> {
    /// The node itself.
    pub fn here() -> Self {
        Self(None)
    }
    /// The node at `below` under the child of the kind `CK` kept under
    /// `key`; the child's position `I` is inferred from its kind.
    pub fn below<CK: NodeKind, I>(key: KeyAt<K::Children, I>, below: KeyAddress<CK>) -> Self
    where
        K::Children: ChildOfKind<CK, I>,
    {
        Self(Some(<K::Children as ChildIndex<I>>::inject_key(key, below)))
    }
    /// The child of the kind `CK` kept under `key`.
    pub fn child<CK: NodeKind, I>(key: KeyAt<K::Children, I>) -> Self
    where
        K::Children: ChildOfKind<CK, I>,
    {
        Self::below::<CK, I>(key, KeyAddress::here())
    }
    /// The key address below the node, or `None` for the node itself.
    pub fn below_here(&self) -> Option<&<K::Children as ChildList>::KeyAddr> {
        self.0.as_ref()
    }
}
impl<K: NodeKind> Clone for KeyAddress<K> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<K: NodeKind> PartialEq for KeyAddress<K> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<K: NodeKind> Eq for KeyAddress<K> {}
impl<K: NodeKind> fmt::Debug for KeyAddress<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyAddress<{}>", core::any::type_name::<K>())
    }
}

/// Something done to the node an address leads to, whose kind is known
/// only when the address is followed.
pub trait Visit<P> {
    /// Visit the node.
    fn node<K: KindFor<P, Children: ChildAddresses<P>>>(&mut self, node: &mut Node<K, P>);
}

/// Following an address below a node, and listing the addresses below it.
pub trait ChildAddresses<P>: ChildStores<P> {
    /// Follow `addr` from the stores; `false` when no node is there.
    fn walk<V: Visit<P>>(stores: &mut Self::Stores, addr: &Self::Addr, v: &mut V) -> bool;
    /// Every address below the stores, each node before the nodes below
    /// it.
    fn each<F: FnMut(Self::Addr)>(stores: &Self::Stores, f: &mut F);
    /// The address of the node a key address below the stores leads to,
    /// when it is there.
    fn resolve(stores: &Self::Stores, key: &Self::KeyAddr) -> Option<Self::Addr>;
    /// Every address below the stores in an order of removal: each node
    /// after the nodes below it, and the children of a kind in `targets`
    /// (kinds other nodes link to) after their siblings' subtrees.
    fn each_for_removal<F: FnMut(Self::Addr)>(stores: &Self::Stores, targets: &[TypeId], f: &mut F);
}
impl<P> ChildAddresses<P> for HNil {
    fn walk<V: Visit<P>>(_: &mut HNil, addr: &CNil, _: &mut V) -> bool {
        match *addr {}
    }
    fn each<F: FnMut(CNil)>(_: &HNil, _: &mut F) {}
    fn resolve(_: &HNil, key: &CNil) -> Option<CNil> {
        match *key {}
    }
    fn each_for_removal<F: FnMut(CNil)>(_: &HNil, _: &[TypeId], _: &mut F) {}
}
impl<P, CK, S: ChildStorage, T: ChildAddresses<P>> ChildAddresses<P> for HCons<Child<CK, S>, T>
where
    CK: KindFor<P, Children: ChildAddresses<P>>,
{
    fn walk<V: Visit<P>>(stores: &mut Self::Stores, addr: &Self::Addr, v: &mut V) -> bool {
        match addr {
            Coproduct::Inl((h, below)) => match stores.head.get_mut(*h) {
                Some(n) => walk_node(n, below, v),
                None => false,
            },
            Coproduct::Inr(r) => T::walk(&mut stores.tail, r, v),
        }
    }
    fn each<F: FnMut(Self::Addr)>(stores: &Self::Stores, f: &mut F) {
        for h in stores.head.handles() {
            f(Coproduct::Inl((h, Address::here())));
            if let Some(n) = stores.head.get(h) {
                CK::Children::each(&n.children, &mut |below| {
                    f(Coproduct::Inl((h, Address::below(below))))
                });
            }
        }
        T::each(&stores.tail, &mut |r| f(Coproduct::Inr(r)));
    }
    fn resolve(stores: &Self::Stores, key: &Self::KeyAddr) -> Option<Self::Addr> {
        match key {
            Coproduct::Inl((k, below)) => {
                let h = stores.head.find(k)?;
                let below = resolve_node(stores.head.get(h)?, below)?;
                Some(Coproduct::Inl((h, below)))
            }
            Coproduct::Inr(r) => T::resolve(&stores.tail, r).map(Coproduct::Inr),
        }
    }
    fn each_for_removal<F: FnMut(Self::Addr)>(
        stores: &Self::Stores,
        targets: &[TypeId],
        f: &mut F,
    ) {
        let head = |f: &mut F| {
            for h in stores.head.handles() {
                if let Some(n) = stores.head.get(h) {
                    CK::Children::each_for_removal(&n.children, targets, &mut |below| {
                        f(Coproduct::Inl((h, Address::below(below))))
                    });
                }
                f(Coproduct::Inl((h, Address::here())));
            }
        };
        let tail =
            |f: &mut F| T::each_for_removal(&stores.tail, targets, &mut |r| f(Coproduct::Inr(r)));
        if targets.contains(&TypeId::of::<CK>()) {
            tail(f);
            head(f);
        } else {
            head(f);
            tail(f);
        }
    }
}

/// Follow `addr` from `n` and visit the node there; `false` when no node is
/// there.
pub fn walk_node<K: KindFor<P, Children: ChildAddresses<P>>, P, V: Visit<P>>(
    n: &mut Node<K, P>,
    addr: &Address<K>,
    v: &mut V,
) -> bool {
    match &addr.0 {
        None => {
            v.node(n);
            true
        }
        Some(a) => K::Children::walk(&mut n.children, a, v),
    }
}

/// The address of the node the key address `key` leads to from `n`, when
/// it is there.
pub fn resolve_node<K: KindFor<P, Children: ChildAddresses<P>>, P>(
    n: &Node<K, P>,
    key: &KeyAddress<K>,
) -> Option<Address<K>> {
    match &key.0 {
        None => Some(Address::here()),
        Some(k) => K::Children::resolve(&n.children, k).map(Address::below),
    }
}

/// Every address at and below `n`, `n` first, each node before the nodes
/// below it.
pub fn addresses_under<K: KindFor<P, Children: ChildAddresses<P>>, P>(
    n: &Node<K, P>,
) -> Vec<Address<K>> {
    let mut out = vec![Address::here()];
    K::Children::each(&n.children, &mut |a| out.push(Address::below(a)));
    out
}

/// Every address at and below the root `n` in the order the driver removes
/// nodes: a node after every node below it, and a node that others link to
/// ([`NodeKind::Linked`]) after the last node that links to it. The root is
/// last.
///
/// The links are by kind: the children of a kind that some kind in the
/// structure links to come after their siblings' subtrees, where the
/// linking nodes are.
pub fn removal_order<K: KindFor<P, Children: ChildAddresses<P>>, P>(
    n: &Node<K, P>,
) -> Vec<Address<K>> {
    let mut targets = Vec::new();
    K::Linked::targets(&mut targets);
    K::Children::link_targets(&mut targets);
    let mut out = Vec::new();
    K::Children::each_for_removal(&n.children, &targets, &mut |a| out.push(Address::below(a)));
    out.push(Address::here());
    out
}

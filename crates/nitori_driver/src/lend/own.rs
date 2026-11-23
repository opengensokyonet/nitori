//! The needs that read the lend point itself: its own key and its address.
use frunk_core::hlist::{HCons, HNil};

use super::{
    Lend,
    tree::{AddressPos, Ctx, KeyPos, SplitKey},
};
use crate::{
    need::{OwnAddress, OwnKey},
    structure::{AtMostOne, ChildIndex, KeyAddress, KindAt, NodeKind, SlotVec, StorageAt},
};

/// The lend point, kept by the storage kind `Self`, is kept under a key of
/// the type `K`.
#[diagnostic::on_unimplemented(
    message = "`OwnKey<{K}>`: the lend point is kept by `{Self}`, not under a key of type `{K}`",
    label = "the lend point has no key of this type",
    note = "the root has no key (`NoParent`); a node kept by `AtMostOne` has the key `()`, by `SlotVec<K>` the key `K`"
)]
pub trait KeyIs<K> {}
impl KeyIs<()> for AtMostOne {}
impl<K> KeyIs<K> for SlotVec<K> {}

impl<C: Ctx, K: 'static> Lend<C, KeyPos> for OwnKey<K>
where
    C::Storage: KeyIs<K> + OwnKeyOf<C::Fk, Key = K>,
{
    type Cols = ();
    fn build<'a>(_: (), at: &C::Fk) -> K
    where
        Self: 'a,
    {
        <C::Storage as OwnKeyOf<C::Fk>>::own_key(at)
    }
}

/// Reading a node's own key from its full key `F`; the node is kept by
/// the storage kind `Self`.
pub trait OwnKeyOf<F> {
    /// The key.
    type Key;
    /// The key, cloned.
    fn own_key(full: &F) -> Self::Key;
}
impl<F> OwnKeyOf<F> for AtMostOne {
    type Key = ();
    fn own_key(_: &F) {}
}
impl<F: super::list::LastKey<Last = K>, K: Clone> OwnKeyOf<F> for SlotVec<K> {
    type Key = K;
    fn own_key(full: &F) -> K {
        full.last_key().clone()
    }
}

impl<C: Ctx> Lend<C, AddressPos<KeyAddress<C::Root, C::Kind>>> for OwnAddress
where
    (C::Root, C::Route): AddressOf<C::Fk, Target = C::Kind, Kind0 = C::Root>,
{
    type Cols = ();
    fn build<'a>(_: (), at: &C::Fk) -> KeyAddress<C::Root, C::Kind>
    where
        Self: 'a,
    {
        <(C::Root, C::Route) as AddressOf<C::Fk>>::address(at)
    }
}

/// The address by keys of the node down the route `R` from a node of the
/// kind `K` (`Self` is `(K, R)`), from what is left `F` of its full key.
pub trait AddressOf<F> {
    /// The kind the route leads to.
    type Target: NodeKind;
    /// The address.
    fn address(full: &F) -> KeyAddress<Self::Kind0, Self::Target>;
    /// The kind the route starts at.
    type Kind0: NodeKind;
}
impl<K: NodeKind> AddressOf<HNil> for (K, HNil) {
    type Target = K;
    type Kind0 = K;
    fn address(_: &HNil) -> KeyAddress<K, K> {
        KeyAddress::here()
    }
}
impl<K: NodeKind, F, J, R> AddressOf<F> for (K, HCons<J, R>)
where
    K::Children: ChildIndex<J>,
    StorageAt<K::Children, J>: SplitKey<F>,
    (KindAt<K::Children, J>, R):
        AddressOf<<StorageAt<K::Children, J> as SplitKey<F>>::Rest, Kind0 = KindAt<K::Children, J>>,
{
    type Target = <(KindAt<K::Children, J>, R) as AddressOf<
        <StorageAt<K::Children, J> as SplitKey<F>>::Rest,
    >>::Target;
    type Kind0 = K;
    fn address(full: &F) -> KeyAddress<K, Self::Target> {
        let (key, rest) = <StorageAt<K::Children, J> as SplitKey<F>>::split_key(full);
        KeyAddress::<K, K>::below_at::<J, _>(
            key,
            <(KindAt<K::Children, J>, R) as AddressOf<_>>::address(rest),
        )
    }
}

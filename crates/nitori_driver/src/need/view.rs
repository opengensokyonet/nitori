//! What a need is lent as: positions, the shapes of a result and the
//! member-by-member access every shape gives.
use alloc::vec::Vec;
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

/// Where a list of needs is lent: what follows from the structure for each
/// need, so that the view's type comes from the need alone, with the
/// position as a parameter ([`Need::View`]).
///
/// The driver infers the position where the structure is known; code that
/// is generic over the structure takes it as an opaque parameter and uses
/// only what is declared here. An item a need does not use is
/// [`EmptyPos`].
pub trait Pos: 'static {
    /// The shape of the result of an [`Under`](super::Under): how many
    /// members it has and how they are told apart.
    type Shape: Shape = Single;
    /// The position of what a need wraps: the members' need of an
    /// [`Under`](super::Under), the need taken at the new lend point of an
    /// [`At`](super::At).
    type Inner: Pos = EmptyPos;
    /// The position of the first need of a list.
    type Head: Pos = EmptyPos;
    /// The position of the other needs of a list.
    type Tail: Pos = EmptyPos;
    /// What [`OwnAddress`](super::OwnAddress) is lent as: the address of
    /// the lend point by keys, from the root.
    type Address: 'static = ();
}

/// The position of nothing: every item of [`Pos`] unused.
pub struct EmptyPos;
impl Pos for EmptyPos {}

impl Pos for HNil {}
impl<H: Pos, T: Pos> Pos for HCons<H, T> {
    type Head = H;
    type Tail = T;
}

/// A need: what is taken from the tree around a lend point, and the view
/// it is lent as at the position `P`.
///
/// The view's type depends on the need, and on the structure only through
/// `P`, so a call written for any structure names its view from its
/// needs.
pub trait Need: 'static {
    /// The view, borrowed for `'t`, at the position `P`.
    type View<'t, P: Pos>;
}

/// The view of the needs `N` at the position `P`, borrowed for `'t`.
pub type View<'t, N, P> = <N as Need>::View<'t, P>;

impl Need for HNil {
    type View<'t, P: Pos> = HNil;
}
impl<N: Need, T: Need> Need for HCons<N, T> {
    type View<'t, P: Pos> = HCons<N::View<'t, P::Head>, T::View<'t, P::Tail>>;
}

/// The shape of the result of an [`Under`](super::Under), which the
/// structure decides: the anchor itself ([`Single`]), at most one node
/// ([`Optional`]), or nodes told apart by keys ([`KeyedBy`]).
pub trait Shape: 'static {
    /// The result, made of one `V` for each member.
    type Out<V>: Members<V>;
}

/// The members are the anchor itself: the result is the member's own.
pub struct Single;
impl Shape for Single {
    type Out<V> = V;
}

/// Every level between the anchor and the members keeps at most one child:
/// the result is there or not.
pub struct Optional;
impl Shape for Optional {
    type Out<V> = Option<V>;
}

/// Some level between the anchor and the members keeps children by key:
/// the result is a [`KeyedView`] by the keys `K` of those levels, the key
/// itself when there is one level and a tuple when there are more.
pub struct KeyedBy<K>(PhantomData<fn() -> K>);
impl<K: PartialEq + 'static> Shape for KeyedBy<K> {
    type Out<V> = KeyedView<K, V>;
}

/// The members of a result, one by one, whatever its shape: a single
/// result is one member under the key `()`.
///
/// The methods have names of their own: every type is the single result
/// of itself, so a method named as one of a collection's would stand for
/// it wherever this trait is imported.
pub trait Members<V> {
    /// What tells the members apart.
    type Key;
    /// How many members there are.
    fn member_count(&self) -> usize;
    /// Every member with its key.
    fn members<'m>(&'m self) -> impl Iterator<Item = (&'m Self::Key, &'m V)>
    where
        V: 'm;
    /// Every member with its key, exclusive.
    fn members_mut<'m>(&'m mut self) -> impl Iterator<Item = (&'m Self::Key, &'m mut V)>
    where
        V: 'm;
    /// The member under `key`.
    fn member(&self, key: &Self::Key) -> Option<&V>;
    /// The member under `key`, exclusive.
    fn member_mut(&mut self, key: &Self::Key) -> Option<&mut V>;
}

impl<V> Members<V> for V {
    type Key = ();
    fn member_count(&self) -> usize {
        1
    }
    fn members<'m>(&'m self) -> impl Iterator<Item = (&'m (), &'m V)>
    where
        V: 'm,
    {
        core::iter::once((&(), self))
    }
    fn members_mut<'m>(&'m mut self) -> impl Iterator<Item = (&'m (), &'m mut V)>
    where
        V: 'm,
    {
        core::iter::once((&(), self))
    }
    fn member(&self, _: &()) -> Option<&V> {
        Some(self)
    }
    fn member_mut(&mut self, _: &()) -> Option<&mut V> {
        Some(self)
    }
}

impl<V> Members<V> for Option<V> {
    type Key = ();
    fn member_count(&self) -> usize {
        usize::from(self.is_some())
    }
    fn members<'m>(&'m self) -> impl Iterator<Item = (&'m (), &'m V)>
    where
        V: 'm,
    {
        Option::as_ref(self).into_iter().map(|v| (&(), v))
    }
    fn members_mut<'m>(&'m mut self) -> impl Iterator<Item = (&'m (), &'m mut V)>
    where
        V: 'm,
    {
        Option::as_mut(self).into_iter().map(|v| (&(), v))
    }
    fn member(&self, _: &()) -> Option<&V> {
        Option::as_ref(self)
    }
    fn member_mut(&mut self, _: &()) -> Option<&mut V> {
        Option::as_mut(self)
    }
}

/// Members told apart by keys: the keys of the levels between the anchor
/// and each member, with what is taken at that member.
///
/// The members are in the order the driver keeps them; finding a key is
/// linear, as it is in the stores.
pub struct KeyedView<K, V> {
    members: Vec<(K, V)>,
}
impl<K, V> KeyedView<K, V> {
    /// A view of `members`, none of them under the same key.
    pub(crate) fn new(members: Vec<(K, V)>) -> Self {
        Self { members }
    }
    /// The keys of the members.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.members.iter().map(|(k, _)| k)
    }
    /// How many members there are.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }
    /// Every member with its key.
    pub fn members(&self) -> impl Iterator<Item = (&K, &V)> {
        self.members.iter().map(|(k, v)| (k, v))
    }
    /// Every member with its key, exclusive.
    pub fn members_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.members.iter_mut().map(|(k, v)| (&*k, v))
    }
}
impl<K: PartialEq, V> KeyedView<K, V> {
    /// The member under `key`.
    pub fn member(&self, key: &K) -> Option<&V> {
        self.members.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    /// The member under `key`, exclusive.
    pub fn member_mut(&mut self, key: &K) -> Option<&mut V> {
        self.members
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
}
impl<K: PartialEq, V> Members<V> for KeyedView<K, V> {
    type Key = K;
    fn member_count(&self) -> usize {
        KeyedView::member_count(self)
    }
    fn members<'m>(&'m self) -> impl Iterator<Item = (&'m K, &'m V)>
    where
        V: 'm,
    {
        KeyedView::members(self)
    }
    fn members_mut<'m>(&'m mut self) -> impl Iterator<Item = (&'m K, &'m mut V)>
    where
        V: 'm,
    {
        KeyedView::members_mut(self)
    }
    fn member(&self, key: &K) -> Option<&V> {
        KeyedView::member(self, key)
    }
    fn member_mut(&mut self, key: &K) -> Option<&mut V> {
        KeyedView::member_mut(self, key)
    }
}

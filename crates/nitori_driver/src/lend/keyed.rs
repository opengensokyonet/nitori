//! Children by key: the cells of a keyed store, what one child gives, the
//! positions of the components in a child's data, and the views.
//!
//! Where a component is in a keyed child's data is a position known only
//! where the structure and the parts are concrete. A view of a keyed need
//! reaches it through a bound ([`KeyedPos`]) on a position type the lender
//! passes, so the code that uses the view names no position.
use core::{convert::Infallible, marker::PhantomData};

use frunk_core::hlist::{HCons, HNil, Sculptor};

use crate::{
    need::HasIn,
    structure::{FlatMut, FlatRef, KindFor, Node},
};

/// The children of one keyed store as lent: all of them (`W` is
/// [`All`](super::All), the store is off the chain) or all but the one on
/// the way up (`W` is [`Others`](super::Others)).
pub struct KeyedCell<'a, W, K, N> {
    before: &'a mut [Option<(K, N)>],
    after: &'a mut [Option<(K, N)>],
    _w: PhantomData<fn() -> W>,
}
impl<'a, W, K: Eq, N> KeyedCell<'a, W, K, N> {
    pub(crate) fn new(before: &'a mut [Option<(K, N)>], after: &'a mut [Option<(K, N)>]) -> Self {
        Self {
            before,
            after,
            _w: PhantomData,
        }
    }
    /// The child kept under `key`.
    pub fn find(&self, key: &K) -> Option<&N> {
        self.iter().find(|(k, _)| *k == key).map(|(_, n)| n)
    }
    /// The child kept under `key`, exclusive.
    pub fn find_mut(&mut self, key: &K) -> Option<&mut N> {
        self.iter_mut().find(|(k, _)| *k == key).map(|(_, n)| n)
    }
    /// Every child as lent, with its key.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &N)> {
        self.before
            .iter()
            .chain(self.after.iter())
            .flatten()
            .map(|(k, n)| (k, n))
    }
    /// Every child as lent, with its key, exclusive.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut N)> {
        self.before
            .iter_mut()
            .chain(self.after.iter_mut())
            .flatten()
            .map(|(k, n)| (&*k, n))
    }
}

/// A keyed cell left after the exclusive needs, shared.
pub struct SharedKeyed<'a, W, K, N> {
    before: &'a [Option<(K, N)>],
    after: &'a [Option<(K, N)>],
    _w: PhantomData<fn() -> W>,
}
impl<W, K, N> Clone for SharedKeyed<'_, W, K, N> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<W, K, N> Copy for SharedKeyed<'_, W, K, N> {}
impl<'a, W, K: Eq, N> SharedKeyed<'a, W, K, N> {
    /// The child kept under `key`.
    pub fn find(&self, key: &K) -> Option<&'a N> {
        self.iter().find(|(k, _)| *k == key).map(|(_, n)| n)
    }
    /// Every child as lent, with its key.
    pub fn iter(&self) -> impl Iterator<Item = (&'a K, &'a N)> + use<'a, W, K, N> {
        self.before
            .iter()
            .chain(self.after.iter())
            .flatten()
            .map(|(k, n)| (k, n))
    }
}
impl<'a, W, K, N> From<KeyedCell<'a, W, K, N>> for SharedKeyed<'a, W, K, N> {
    fn from(c: KeyedCell<'a, W, K, N>) -> Self {
        Self {
            before: c.before,
            after: c.after,
            _w: PhantomData,
        }
    }
}

// ---------------------------------------------------------------- keyed items

/// What a keyed need takes: its key, and what one child gives, shared and
/// exclusive.
pub trait KeyedDesc {
    /// The key of the children.
    type Key;
    /// What one child gives to a shared borrow.
    type Ref<'r>
    where
        Self: 'r;
    /// What one child gives to an exclusive borrow (shared for a shared
    /// need).
    type Mut<'r>
    where
        Self: 'r;
}
/// An exclusive keyed need of the component `X`, the word `W`, the key
/// `K`.
#[allow(clippy::type_complexity)]
pub struct KeyedItem<W, K, X>(PhantomData<fn() -> (W, K, X)>);
impl<W, K, X> KeyedDesc for KeyedItem<W, K, X> {
    type Key = K;
    type Ref<'r>
        = &'r X
    where
        Self: 'r;
    type Mut<'r>
        = &'r mut X
    where
        Self: 'r;
}
/// A shared keyed need of the component `X`, the word `W`, the key `K`.
#[allow(clippy::type_complexity)]
pub struct KeyedRefItem<W, K, X>(PhantomData<fn() -> (W, K, X)>);
impl<W, K, X> KeyedDesc for KeyedRefItem<W, K, X> {
    type Key = K;
    type Ref<'r>
        = &'r X
    where
        Self: 'r;
    type Mut<'r>
        = &'r X
    where
        Self: 'r;
}
/// An exclusive keyed need of the components of the list `L`, the word
/// `W`, the key `K`.
#[allow(clippy::type_complexity)]
pub struct KeyedMany<W, K, L>(PhantomData<fn() -> (W, K, L)>);
impl<W, K, L: Refs> KeyedDesc for KeyedMany<W, K, L> {
    type Key = K;
    type Ref<'r>
        = L::Ref<'r>
    where
        Self: 'r;
    type Mut<'r>
        = L::Mut<'r>
    where
        Self: 'r;
}
/// The head of an empty list of keyed needs.
pub struct NoKeyed;
impl KeyedDesc for NoKeyed {
    type Key = ();
    type Ref<'r>
        = ()
    where
        Self: 'r;
    type Mut<'r>
        = ()
    where
        Self: 'r;
}

/// A list of component types, borrowed one by one.
pub trait Refs {
    /// The list of `&` components.
    type Ref<'r>
    where
        Self: 'r;
    /// The list of `&mut` components.
    type Mut<'r>
    where
        Self: 'r;
}
impl Refs for HNil {
    type Ref<'r>
        = HNil
    where
        Self: 'r;
    type Mut<'r>
        = HNil
    where
        Self: 'r;
}
impl<H, T: Refs> Refs for HCons<H, T> {
    type Ref<'r>
        = HCons<&'r H, T::Ref<'r>>
    where
        Self: 'r;
    type Mut<'r>
        = HCons<&'r mut H, T::Mut<'r>>
    where
        Self: 'r;
}

/// A list of keyed needs as head and tail (the empty list: [`NoKeyed`]
/// and itself).
pub trait KeyedShape {
    /// The first keyed need.
    type Head: KeyedDesc;
    /// The others.
    type Tail: KeyedShape;
}
impl KeyedShape for HNil {
    type Head = NoKeyed;
    type Tail = HNil;
}
impl<H: KeyedDesc, T: KeyedShape> KeyedShape for HCons<H, T> {
    type Head = H;
    type Tail = T;
}

// ---------------------------------------------------------------- positions

/// How the keyed need `N` is read for the parts `P`: the store as lent,
/// and what a child gives by key.
pub trait KeyedPos<P, N: KeyedDesc> {
    /// The store as lent.
    type Raw<'w>;
    /// The child kept under `key`.
    fn get<'r>(raw: &'r Self::Raw<'_>, key: &N::Key) -> Option<N::Ref<'r>>
    where
        N: 'r;
    /// Every child as lent, with its key.
    fn iter<'r, 'w: 'r>(raw: &'r Self::Raw<'w>) -> impl Iterator<Item = (&'r N::Key, N::Ref<'r>)>
    where
        N: 'r,
        N::Key: 'r;
    /// The child kept under `key`, exclusive.
    fn get_mut<'r>(raw: &'r mut Self::Raw<'_>, key: &N::Key) -> Option<N::Mut<'r>>
    where
        N: 'r;
    /// Every child as lent, with its key, exclusive.
    fn iter_mut<'r, 'w: 'r>(
        raw: &'r mut Self::Raw<'w>,
    ) -> impl Iterator<Item = (&'r N::Key, N::Mut<'r>)>
    where
        N: 'r,
        N::Key: 'r;
}
/// The positions of a list of keyed needs, head and tail.
pub trait KeyedList<P, L: KeyedShape> {
    /// The position of the first keyed need.
    type Head: KeyedPos<P, L::Head>;
    /// The positions of the others.
    type Tail: KeyedList<P, L::Tail>;
}
/// The position past the end of a list.
pub struct NoKeyedPos;
impl<P> KeyedPos<P, NoKeyed> for NoKeyedPos {
    type Raw<'w> = Infallible;
    fn get<'r>(raw: &'r Infallible, _: &()) -> Option<()>
    where
        NoKeyed: 'r,
    {
        match *raw {}
    }
    fn iter<'r, 'w: 'r>(raw: &'r Infallible) -> impl Iterator<Item = (&'r (), ())>
    where
        NoKeyed: 'r,
    {
        let _ = raw;
        core::iter::empty()
    }
    fn get_mut<'r>(raw: &'r mut Infallible, _: &()) -> Option<()>
    where
        NoKeyed: 'r,
    {
        match *raw {}
    }
    fn iter_mut<'r, 'w: 'r>(raw: &'r mut Infallible) -> impl Iterator<Item = (&'r (), ())>
    where
        NoKeyed: 'r,
    {
        let _ = raw;
        core::iter::empty()
    }
}
impl<P> KeyedList<P, HNil> for HNil {
    type Head = NoKeyedPos;
    type Tail = HNil;
}
impl<P, H: KeyedDesc, T: KeyedShape, PH, PT> KeyedList<P, HCons<H, T>> for HCons<PH, PT>
where
    PH: KeyedPos<P, H>,
    PT: KeyedList<P, T>,
{
    type Head = PH;
    type Tail = PT;
}

/// The position of a keyed need of one component: the child kind `B` and
/// where the component is in its data (`CI`).
pub struct KeyedP<B, CI>(PhantomData<fn() -> (B, CI)>);
impl<P: 'static, W, K: Eq + 'static, X, B, CI> KeyedPos<P, KeyedItem<W, K, X>> for KeyedP<B, CI>
where
    B: KindFor<P>,
    B::Data: HasIn<X, CI>,
{
    type Raw<'w> = KeyedCell<'w, W, K, Node<B, P>>;
    fn get<'r>(raw: &'r Self::Raw<'_>, key: &K) -> Option<&'r X>
    where
        KeyedItem<W, K, X>: 'r,
    {
        Some(raw.find(key)?.data.get_in())
    }
    fn iter<'r, 'w: 'r>(raw: &'r Self::Raw<'w>) -> impl Iterator<Item = (&'r K, &'r X)>
    where
        KeyedItem<W, K, X>: 'r,
        K: 'r,
    {
        raw.iter().map(|(k, n)| (k, n.data.get_in()))
    }
    fn get_mut<'r>(raw: &'r mut Self::Raw<'_>, key: &K) -> Option<&'r mut X>
    where
        KeyedItem<W, K, X>: 'r,
    {
        Some(raw.find_mut(key)?.data.get_in_mut())
    }
    fn iter_mut<'r, 'w: 'r>(raw: &'r mut Self::Raw<'w>) -> impl Iterator<Item = (&'r K, &'r mut X)>
    where
        KeyedItem<W, K, X>: 'r,
        K: 'r,
    {
        raw.iter_mut().map(|(k, n)| (k, n.data.get_in_mut()))
    }
}
impl<P: 'static, W, K: Eq + 'static, X, B, CI> KeyedPos<P, KeyedRefItem<W, K, X>> for KeyedP<B, CI>
where
    B: KindFor<P>,
    B::Data: HasIn<X, CI>,
{
    type Raw<'w> = SharedKeyed<'w, W, K, Node<B, P>>;
    fn get<'r>(raw: &'r Self::Raw<'_>, key: &K) -> Option<&'r X>
    where
        KeyedRefItem<W, K, X>: 'r,
    {
        Some(raw.find(key)?.data.get_in())
    }
    fn iter<'r, 'w: 'r>(raw: &'r Self::Raw<'w>) -> impl Iterator<Item = (&'r K, &'r X)>
    where
        KeyedRefItem<W, K, X>: 'r,
        K: 'r,
    {
        raw.iter().map(|(k, n)| (k, n.data.get_in()))
    }
    fn get_mut<'r>(raw: &'r mut Self::Raw<'_>, key: &K) -> Option<&'r X>
    where
        KeyedRefItem<W, K, X>: 'r,
    {
        Some(raw.find(key)?.data.get_in())
    }
    fn iter_mut<'r, 'w: 'r>(raw: &'r mut Self::Raw<'w>) -> impl Iterator<Item = (&'r K, &'r X)>
    where
        KeyedRefItem<W, K, X>: 'r,
        K: 'r,
    {
        raw.iter().map(|(k, n)| (k, n.data.get_in()))
    }
}

type FlatM<'r, B, P> = <<B as KindFor<P>>::Data as FlatMut<'r>>::Out;
type FlatR<'r, B, P> = <<B as KindFor<P>>::Data as FlatRef<'r>>::Out;

/// The position of a keyed need of several components: the child kind `B`
/// and where the components are in its data borrowed flat (a frunk
/// `Sculptor`'s indices).
pub struct KeyedManyP<B, CI>(PhantomData<fn() -> (B, CI)>);
impl<P: 'static, W, K, L, B, CI> KeyedPos<P, KeyedMany<W, K, L>> for KeyedManyP<B, CI>
where
    K: Eq + 'static,
    L: Refs + 'static,
    B: KindFor<P>,
    for<'r> B::Data: FlatMut<'r> + FlatRef<'r>,
    for<'r> FlatM<'r, B, P>: Sculptor<L::Mut<'r>, CI>,
    for<'r> FlatR<'r, B, P>: Sculptor<L::Ref<'r>, CI>,
{
    type Raw<'w> = KeyedCell<'w, W, K, Node<B, P>>;
    fn get<'r>(raw: &'r Self::Raw<'_>, key: &K) -> Option<L::Ref<'r>>
    where
        KeyedMany<W, K, L>: 'r,
    {
        let flat = raw.find(key)?.data.flat_ref();
        Some(<FlatR<'r, B, P> as Sculptor<L::Ref<'r>, CI>>::sculpt(flat).0)
    }
    fn iter<'r, 'w: 'r>(raw: &'r Self::Raw<'w>) -> impl Iterator<Item = (&'r K, L::Ref<'r>)>
    where
        KeyedMany<W, K, L>: 'r,
        K: 'r,
    {
        raw.iter().map(|(k, n)| {
            let flat = n.data.flat_ref();
            (
                k,
                <FlatR<'r, B, P> as Sculptor<L::Ref<'r>, CI>>::sculpt(flat).0,
            )
        })
    }
    fn get_mut<'r>(raw: &'r mut Self::Raw<'_>, key: &K) -> Option<L::Mut<'r>>
    where
        KeyedMany<W, K, L>: 'r,
    {
        let flat = raw.find_mut(key)?.data.flat_mut();
        Some(<FlatM<'r, B, P> as Sculptor<L::Mut<'r>, CI>>::sculpt(flat).0)
    }
    fn iter_mut<'r, 'w: 'r>(raw: &'r mut Self::Raw<'w>) -> impl Iterator<Item = (&'r K, L::Mut<'r>)>
    where
        KeyedMany<W, K, L>: 'r,
        K: 'r,
    {
        raw.iter_mut().map(|(k, n)| {
            let flat = n.data.flat_mut();
            (
                k,
                <FlatM<'r, B, P> as Sculptor<L::Mut<'r>, CI>>::sculpt(flat).0,
            )
        })
    }
}

// ---------------------------------------------------------------- views

/// The view of an exclusive keyed need: the children of a store by key.
/// With [`ByKeyOthers`](crate::need::ByKeyOthers) the node on the way up is
/// not among them, so its own key finds `None`.
///
/// A child gives `&mut X` for a need [`Mut<X>`](crate::need::Mut), and the
/// list `HList![&mut A, &mut B]` for a need
/// [`MutList<HList![A, B]>`](crate::need::MutList).
pub struct Keyed<'w, P, N: KeyedDesc, KP: KeyedPos<P, N>> {
    raw: KP::Raw<'w>,
    _item: PhantomData<fn() -> (P, N)>,
}
impl<'w, P, N: KeyedDesc, KP: KeyedPos<P, N>> Keyed<'w, P, N, KP> {
    pub(crate) fn new(raw: KP::Raw<'w>) -> Self {
        Self {
            raw,
            _item: PhantomData,
        }
    }
    /// The child kept under `key`.
    pub fn get(&self, key: &N::Key) -> Option<N::Ref<'_>> {
        KP::get(&self.raw, key)
    }
    /// The child kept under `key`, exclusive.
    pub fn get_mut(&mut self, key: &N::Key) -> Option<N::Mut<'_>> {
        KP::get_mut(&mut self.raw, key)
    }
    /// Every child, with its key.
    pub fn iter(&self) -> impl Iterator<Item = (&N::Key, N::Ref<'_>)> {
        KP::iter(&self.raw)
    }
    /// Every child, with its key, exclusive.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&N::Key, N::Mut<'_>)> {
        KP::iter_mut(&mut self.raw)
    }
    /// The children kept under several keys at once, exclusive, in the
    /// order of `keys`. A key not there finds `None`, and so does a key
    /// after its first place in `keys`.
    pub fn get_many_mut<const M: usize>(&mut self, keys: [&N::Key; M]) -> [Option<N::Mut<'_>>; M]
    where
        N::Key: PartialEq,
    {
        let mut out = [const { None }; M];
        for (k, x) in self.iter_mut() {
            if let Some(i) = keys.iter().position(|q| *q == k) {
                out[i] = Some(x);
            }
        }
        out
    }
}

/// The view of a shared keyed need: the children of a store by key, shared
/// with the other shared needs of them.
pub struct KeyedRef<'w, P, N: KeyedDesc, KP: KeyedPos<P, N>> {
    raw: KP::Raw<'w>,
    _item: PhantomData<fn() -> (P, N)>,
}
impl<'w, P, N: KeyedDesc, KP: KeyedPos<P, N>> KeyedRef<'w, P, N, KP> {
    pub(crate) fn new(raw: KP::Raw<'w>) -> Self {
        Self {
            raw,
            _item: PhantomData,
        }
    }
    /// The child kept under `key`.
    pub fn get(&self, key: &N::Key) -> Option<N::Ref<'_>> {
        KP::get(&self.raw, key)
    }
    /// Every child, with its key.
    pub fn iter(&self) -> impl Iterator<Item = (&N::Key, N::Ref<'_>)> {
        KP::iter(&self.raw)
    }
}

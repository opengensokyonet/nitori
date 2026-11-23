//! Type-level lists, counts and answers, and the full keys of nodes.
//!
//! A full key is the keys from the root down to a node, one per level kept
//! by key, the root's first: an `HList`. A level that keeps at most one
//! child adds no key.
use core::marker::PhantomData;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use crate::id::{False, True};

/// The count zero.
pub struct Z;
/// One more than `N`.
pub struct S<N>(PhantomData<fn() -> N>);

/// The length of a type-level list.
pub trait Len {
    /// The length.
    type N;
}
impl Len for HNil {
    type N = Z;
}
impl<H, T: Len> Len for HCons<H, T> {
    type N = S<T::N>;
}

/// The sum of two counts.
pub trait Sum<M> {
    /// The sum.
    type Out;
}
impl<M> Sum<M> for Z {
    type Out = M;
}
impl<N: Sum<M>, M> Sum<M> for S<N> {
    type Out = S<N::Out>;
}

/// Both answers yes.
pub trait And<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl<B> And<B> for True {
    type Out = B;
}
impl<B> And<B> for False {
    type Out = False;
}

/// Either answer yes.
pub trait Or<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl<B> Or<B> for True {
    type Out = True;
}
impl<B> Or<B> for False {
    type Out = B;
}

/// One more when [`True`].
pub trait IncIf<N> {
    /// `N` or one more.
    type Out;
}
impl<N> IncIf<N> for True {
    type Out = S<N>;
}
impl<N> IncIf<N> for False {
    type Out = N;
}

/// A type-level list with `B` after it.
pub trait Append<B> {
    /// The longer list.
    type Out;
}
impl<B> Append<B> for HNil {
    type Out = B;
}
impl<H, T: Append<B>, B> Append<B> for HCons<H, T> {
    type Out = HCons<H, T::Out>;
}

/// A route (a list of positions among children, from the root down) with
/// its first position one further: the same route seen from the list of
/// children one earlier.
pub trait ShiftFirst {
    /// The shifted route.
    type Out;
}
impl<J, R> ShiftFirst for HCons<J, R> {
    type Out = HCons<There<J>, R>;
}

/// Whether two positions are the same.
pub trait IndexEq<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl IndexEq<Here> for Here {
    type Out = True;
}
impl<J> IndexEq<There<J>> for Here {
    type Out = False;
}
impl<I> IndexEq<Here> for There<I> {
    type Out = False;
}
impl<I: IndexEq<J>, J> IndexEq<There<J>> for There<I> {
    type Out = I::Out;
}

/// Whether two routes lead to the same kind.
pub trait RouteEq<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl RouteEq<HNil> for HNil {
    type Out = True;
}
impl<J, R> RouteEq<HCons<J, R>> for HNil {
    type Out = False;
}
impl<I, Q> RouteEq<HNil> for HCons<I, Q> {
    type Out = False;
}
impl<I: IndexEq<J>, Q: RouteEq<R>, J, R> RouteEq<HCons<J, R>> for HCons<I, Q>
where
    I::Out: And<Q::Out>,
{
    type Out = <I::Out as And<Q::Out>>::Out;
}

/// A full key with the key `k` of one more level.
pub trait PushKey<K> {
    /// The longer key.
    type Out;
    /// The longer key, `self` cloned.
    fn push_key(&self, k: K) -> Self::Out;
}
impl<K> PushKey<K> for HNil {
    type Out = HCons<K, HNil>;
    fn push_key(&self, k: K) -> Self::Out {
        HCons {
            head: k,
            tail: HNil,
        }
    }
}
impl<H: Clone, T: PushKey<K>, K> PushKey<K> for HCons<H, T> {
    type Out = HCons<H, T::Out>;
    fn push_key(&self, k: K) -> Self::Out {
        HCons {
            head: self.head.clone(),
            tail: self.tail.push_key(k),
        }
    }
}

/// The full key of an ancestor, `P`, cloned from the full key of a node.
pub trait Prefix<P> {
    /// The ancestor's full key.
    fn prefix(&self) -> P;
}
impl<L> Prefix<HNil> for L {
    fn prefix(&self) -> HNil {
        HNil
    }
}
impl<H: Clone, T: Prefix<P>, P> Prefix<HCons<H, P>> for HCons<H, T> {
    fn prefix(&self) -> HCons<H, P> {
        HCons {
            head: self.head.clone(),
            tail: self.tail.prefix(),
        }
    }
}

/// Whether a full key starts with the full key `P` of an ancestor's type.
pub trait HasPrefix<P> {
    /// Whether it does.
    fn has_prefix(&self, p: &P) -> bool;
}
impl<L> HasPrefix<HNil> for L {
    fn has_prefix(&self, _: &HNil) -> bool {
        true
    }
}
impl<H: PartialEq, T: HasPrefix<Q>, Q> HasPrefix<HCons<H, Q>> for HCons<H, T> {
    fn has_prefix(&self, p: &HCons<H, Q>) -> bool {
        self.head == p.head && self.tail.has_prefix(&p.tail)
    }
}

/// A full key `Self` followed by the keys `Sx`, and back.
pub trait Concat<Sx> {
    /// The longer key.
    type Out;
    /// The keys after `p` in `full`, when `full` starts with `p`.
    fn strip<'f>(full: &'f Self::Out, p: &Self) -> Option<&'f Sx>;
}
impl<Sx> Concat<Sx> for HNil {
    type Out = Sx;
    fn strip<'f>(full: &'f Sx, _: &HNil) -> Option<&'f Sx> {
        Some(full)
    }
}
impl<H: PartialEq, T: Concat<Sx>, Sx> Concat<Sx> for HCons<H, T> {
    type Out = HCons<H, T::Out>;
    fn strip<'f>(full: &'f Self::Out, p: &Self) -> Option<&'f Sx> {
        if full.head == p.head {
            T::strip(&full.tail, &p.tail)
        } else {
            None
        }
    }
}

/// The keys of the levels between an anchor and its members, as the key
/// of a keyed view: the key itself for one level, a tuple for more.
pub trait KeyTuple {
    /// The key of the view.
    type Tuple: Clone + PartialEq + 'static;
    /// The keys, cloned.
    fn to_tuple(&self) -> Self::Tuple;
}
impl<A: Clone + PartialEq + 'static> KeyTuple for HCons<A, HNil> {
    type Tuple = A;
    fn to_tuple(&self) -> A {
        self.head.clone()
    }
}
impl<A, B> KeyTuple for HCons<A, HCons<B, HNil>>
where
    A: Clone + PartialEq + 'static,
    B: Clone + PartialEq + 'static,
{
    type Tuple = (A, B);
    fn to_tuple(&self) -> (A, B) {
        (self.head.clone(), self.tail.head.clone())
    }
}
impl<A, B, C> KeyTuple for HCons<A, HCons<B, HCons<C, HNil>>>
where
    A: Clone + PartialEq + 'static,
    B: Clone + PartialEq + 'static,
    C: Clone + PartialEq + 'static,
{
    type Tuple = (A, B, C);
    fn to_tuple(&self) -> (A, B, C) {
        (
            self.head.clone(),
            self.tail.head.clone(),
            self.tail.tail.head.clone(),
        )
    }
}
impl<A, B, C, D> KeyTuple for HCons<A, HCons<B, HCons<C, HCons<D, HNil>>>>
where
    A: Clone + PartialEq + 'static,
    B: Clone + PartialEq + 'static,
    C: Clone + PartialEq + 'static,
    D: Clone + PartialEq + 'static,
{
    type Tuple = (A, B, C, D);
    fn to_tuple(&self) -> (A, B, C, D) {
        (
            self.head.clone(),
            self.tail.head.clone(),
            self.tail.tail.head.clone(),
            self.tail.tail.tail.head.clone(),
        )
    }
}

/// The last key of a full key that has at least one.
pub trait LastKey {
    /// Its type.
    type Last;
    /// The last key.
    fn last_key(&self) -> &Self::Last;
}
impl<H> LastKey for HCons<H, HNil> {
    type Last = H;
    fn last_key(&self) -> &H {
        &self.head
    }
}
impl<H, H2, T> LastKey for HCons<H, HCons<H2, T>>
where
    HCons<H2, T>: LastKey,
{
    type Last = <HCons<H2, T> as LastKey>::Last;
    fn last_key(&self) -> &Self::Last {
        self.tail.last_key()
    }
}

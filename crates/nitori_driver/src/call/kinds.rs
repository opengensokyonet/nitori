//! What a composition reads off its structure: every kind, where each is,
//! that none is at two places, the calls and components of each kind, and
//! a request box for each kind.
use alloc::vec::Vec;
use core::ops::Add;

use frunk_core::hlist::{HCons, HList, HNil};

pub use crate::lend::react::{AllKinds, KidKinds};
use crate::{
    id::{False, NumberEq, Numbered, True},
    lend::{
        context::{Pieces, RequestList},
        tree::{InKid, IsSelf, KHead, KTail},
    },
    request::NodeRequest,
    structure::{Child, KeyRoute, NodeKind, Share},
};

// ------------------------------------------------------------------ where a kind is

/// Where the kind `K` is at or below the kind `Self`, found by the kinds'
/// numbers: `Il` is the way down to it a lending takes.
pub trait FindKind<K> {
    /// The way down.
    type Il;
}
impl<S: NodeKind + Numbered, K: Numbered> FindKind<K> for S
where
    S::Number: NumberEq<K::Number>,
    <S::Number as NumberEq<K::Number>>::Out: FindAt<S, K>,
{
    type Il = <<S::Number as NumberEq<K::Number>>::Out as FindAt<S, K>>::Il;
}
/// The way down to `K` from `S`, `Self` saying whether `S` is `K`.
pub trait FindAt<S, K> {
    /// The way down.
    type Il;
}
impl<S, K> FindAt<S, K> for True {
    type Il = IsSelf;
}
impl<S: NodeKind, K> FindAt<S, K> for False
where
    S::Children: FindInKids<K>,
{
    type Il = InKid<<S::Children as FindInKids<K>>::I>;
}
/// The way down to `K` below a list of children: under the first child
/// that has it at or below it.
pub trait FindInKids<K> {
    /// The way down, from the list's first position.
    type I;
}
impl<K, C: HasKind<K>, St, T> FindInKids<K> for HCons<Child<C, St>, T>
where
    C::Out: PickKid<C, T, K>,
{
    type I = <C::Out as PickKid<C, T, K>>::I;
}
/// Under the child `C` (`Self` is [`True`]) or under one of the
/// children `T` after it.
pub trait PickKid<C, T, K> {
    /// The way down.
    type I;
}
impl<C: FindKind<K>, T, K> PickKid<C, T, K> for True {
    type I = KHead<C::Il>;
}
impl<C, T: FindInKids<K>, K> PickKid<C, T, K> for False {
    type I = KTail<T::I>;
}
/// Whether the kind `K` is at or below the kind `Self`: [`True`] or
/// [`False`].
pub trait HasKind<K> {
    /// The answer.
    type Out;
}
impl<S: AllKinds, K> HasKind<K> for S
where
    S::Kinds: AnyIs<K>,
{
    type Out = <S::Kinds as AnyIs<K>>::Out;
}
/// Whether one of the kinds `Self` is `K`.
pub trait AnyIs<K> {
    /// The answer.
    type Out;
}
impl<K> AnyIs<K> for HNil {
    type Out = False;
}
impl<K: Numbered, H: Numbered, T: AnyIs<K>> AnyIs<K> for HCons<H, T>
where
    H::Number: NumberEq<K::Number>,
    <H::Number as NumberEq<K::Number>>::Out: Or<T::Out>,
{
    type Out = <<H::Number as NumberEq<K::Number>>::Out as Or<T::Out>>::Out;
}
/// `Self` or `B`.
pub trait Or<B> {
    /// The answer.
    type Out;
}
impl<B> Or<B> for True {
    type Out = True;
}
impl<B> Or<B> for False {
    type Out = B;
}

// ------------------------------------------------------------------ one place each

/// Every kind of the list `Self` is in it once: the kinds of a structure
/// are each at one place.
pub trait KindsOnce {}
impl KindsOnce for HNil {}
impl<K: Numbered, T: KindsOnce + KindNotIn<K>> KindsOnce for HCons<K, T> {}
/// The kind `K` is none of the kinds `Self`.
pub(crate) trait KindNotIn<K> {}
impl<K> KindNotIn<K> for HNil {}
impl<K: Numbered, H: Numbered, T: KindNotIn<K>> KindNotIn<K> for HCons<H, T>
where
    H::Number: NumberEq<K::Number>,
    <H::Number as NumberEq<K::Number>>::Out: OtherKind<K>,
{
}
/// Another kind than `K`: [`False`] only.
#[diagnostic::on_unimplemented(
    message = "the node kind `{K}` is at two places of the structure",
    label = "`{K}` comes again",
    note = "a kind names the same children wherever it is used, and the lend points and the requests of a structure are told apart by kind: give each place its own kind"
)]
pub trait OtherKind<K> {}
impl<K> OtherKind<K> for False {}

// ------------------------------------------------------------------ lists

/// A list of types as what follows its first item: an empty list is
/// followed by itself.
pub trait ListRest {
    /// What follows the first item.
    type Rest: ListRest;
}
impl ListRest for HNil {
    type Rest = HNil;
}
impl<H, T: ListRest> ListRest for HCons<H, T> {
    type Rest = T;
}

/// The calls of a list of parts `Self` at the kind `K`: every part's
/// [`Share::Calls`], in the order of the parts.
pub trait PartsCalls<K> {
    /// The kinds of calls.
    type Calls: HList + ListRest;
}
impl<K> PartsCalls<K> for HNil {
    type Calls = HNil;
}
impl<K, O: Share<K>, Os: PartsCalls<K>> PartsCalls<K> for HCons<O, Os>
where
    O::Calls: Add<Os::Calls, Output: HList + ListRest>,
{
    type Calls = <O::Calls as Add<Os::Calls>>::Output;
}

/// The component `C` in the share of the part `O`.
pub struct Comp<O, C>(core::marker::PhantomData<fn() -> (O, C)>);
/// The components `Self` of the part `O`, each as a [`Comp`].
pub trait InPart<O> {
    /// The list.
    type Out: HList;
}
impl<O> InPart<O> for HNil {
    type Out = HNil;
}
impl<O, C, T: InPart<O>> InPart<O> for HCons<C, T> {
    type Out = HCons<Comp<O, C>, T::Out>;
}
/// The components of a list of parts `Self` at the kind `K`, each as a
/// [`Comp`], in the order of the parts.
pub trait PartsComps<K> {
    /// The components.
    type Comps: HList + ListRest;
}
impl<K> PartsComps<K> for HNil {
    type Comps = HNil;
}
impl<K, O: Share<K, Data: InPart<O>>, Os: PartsComps<K>> PartsComps<K> for HCons<O, Os>
where
    <O::Data as InPart<O>>::Out: Add<Os::Comps, Output: HList + ListRest>,
{
    type Comps = <<O::Data as InPart<O>>::Out as Add<Os::Comps>>::Output;
}

// ------------------------------------------------------------------ request boxes

/// A box for the requests of every kind `Self` of the structure whose
/// root is of the kind `R`, in the order of the kinds.
pub trait RequestLists<R: NodeKind> {
    /// The boxes.
    type Lists: Pieces + RequestQueues<R>;
    /// Every box, empty.
    fn lists() -> Self::Lists;
}
impl<R: NodeKind> RequestLists<R> for HNil {
    type Lists = HNil;
    fn lists() -> HNil {
        HNil
    }
}
/// The request boxes `Self` of every kind of a structure whose root is
/// of the kind `R`, as the driver handles them: by the number of the
/// kind, counted from the first box.
pub trait RequestQueues<R: NodeKind> {
    /// Queue the requests put in so far, and note every one queued with
    /// its place in the order and its kind, the first box being `kind`.
    fn enqueue_all(&mut self, kind: usize, out: &mut Vec<(u64, usize)>);
    /// The route of the first request queued in the box `kind`, with its
    /// place in the order.
    fn first(&self, kind: usize) -> Option<(u64, &KeyRoute<R>)>;
    /// Drop every request in every box.
    fn clear_all(&mut self);
}
impl<R: NodeKind> RequestQueues<R> for HNil {
    fn enqueue_all(&mut self, _: usize, _: &mut Vec<(u64, usize)>) {}
    fn first(&self, _: usize) -> Option<(u64, &KeyRoute<R>)> {
        None
    }
    fn clear_all(&mut self) {}
}
impl<R: NodeKind, K: NodeKind, T: RequestQueues<R>> RequestQueues<R>
    for HCons<RequestList<NodeRequest<R, K>>, T>
{
    fn enqueue_all(&mut self, kind: usize, out: &mut Vec<(u64, usize)>) {
        self.head.enqueue();
        out.extend(self.head.queued().map(|order| (order, kind)));
        self.tail.enqueue_all(kind + 1, out);
    }
    fn first(&self, kind: usize) -> Option<(u64, &KeyRoute<R>)> {
        match kind.checked_sub(1) {
            None => self.head.first().map(|(order, r)| (*order, r.at.route())),
            Some(rest) => self.tail.first(rest),
        }
    }
    fn clear_all(&mut self) {
        self.head.clear();
        self.tail.clear_all();
    }
}

impl<R: NodeKind, K: NodeKind, T: RequestLists<R>> RequestLists<R> for HCons<K, T> {
    type Lists = HCons<RequestList<NodeRequest<R, K>>, T::Lists>;
    fn lists() -> Self::Lists {
        HCons {
            head: RequestList::new(),
            tail: T::lists(),
        }
    }
}

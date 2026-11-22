//! The words of a need: what something lent at a node asks of the tree
//! around it.
//!
//! A need is [`At`]`<anchor, steps, take>`. The anchor is a node on the
//! way up from the node lent (the chain: the node itself, its parent, and
//! so on to the root), found by its kind ([`Kind`]), by how far up it is
//! ([`Up`]) or by a component only it has ([`Provides`]). The steps go down
//! from the anchor ([`Here`], [`ToChild`], [`Then`]). What is taken is a
//! component, exclusive ([`Mut`]) or shared ([`Ref`]), or several
//! components of every child by key ([`MutList`]); or, with the steps
//! [`Here`], the instant the driver read when it was last entered
//! ([`Now`]).
//!
//! A list of needs is proved where it is lent on a concrete structure:
//! the shape at compile time, the presence of a node reached through an
//! at-most-one child or by key at run time (an `Option`). A need names
//! kinds and components only; it holds for any structure of that shape.
use core::{marker::PhantomData, ops::Add};

use frunk_core::{
    hlist::{HCons, HNil, Selector},
    indices::{Here as I0, There},
};

use crate::structure::{
    AtMostOne, ChildIndex, ChildOfKind, ChildStorage, End, KindAt, KindFor, NodeKind, Step,
};

/// One need: from the anchor `A`, the steps `S`, take `T`.
#[allow(clippy::type_complexity)]
pub struct At<A, S, T>(PhantomData<fn() -> (A, S, T)>);

/// Anchor: the node of the kind `A` on the way up, the node lent included.
///
/// It binds the need to the kind `A`; two nodes of the kind on the way up
/// leave the position ambiguous, which does not compile.
pub struct Kind<A>(PhantomData<fn() -> A>);
/// Anchor: `N` levels up (`Up<0>` is the node lent), for `N` up to 5.
pub struct Up<const N: usize>;
/// Anchor: the one node on the way up whose data has the component `X`.
pub struct Provides<X>(PhantomData<fn() -> X>);

/// No step: the anchor itself.
pub struct Here;
/// A step to the child of the kind `B`; `M` says which ([`AtMost`] by
/// default).
pub struct ToChild<B, M = AtMost>(PhantomData<fn() -> (B, M)>);
/// One step, then the steps `R`.
pub struct Then<S, R>(PhantomData<fn() -> (S, R)>);

/// The child kept as at most one: the view is an `Option`.
pub struct AtMost;
/// The child the node lent links to by its own key ([`ByOwnKey`]). It is
/// there whenever the node is, so the view is not an `Option`.
///
/// [`ByOwnKey`]: crate::structure::ByOwnKey
pub struct MyLink;
/// Every child of the kind, by a key of the type `K` at run time; only on
/// a store off the chain (no node on the way up is kept in it).
pub struct ByKey<K>(PhantomData<fn() -> K>);
/// Every child of the kind but the one on the way up, by a key of the type
/// `K` at run time; only on a store on the chain.
pub struct ByKeyOthers<K>(PhantomData<fn() -> K>);

/// Take the component `X`, exclusive.
pub struct Mut<X>(PhantomData<fn() -> X>);
/// Take the component `X`, shared with the other shared needs of it.
pub struct Ref<X>(PhantomData<fn() -> X>);
/// Take the components of the list `L` of every child by key, exclusive;
/// only after a [`ByKey`] or [`ByKeyOthers`] step. A child gives the list
/// of `&mut` components, in the order of `L`.
pub struct MutList<L>(PhantomData<fn() -> L>);
/// Take the instant the driver read when it was last entered, of the
/// type `I`, which is its clock's (the composition checks it); only with
/// the steps [`Here`]. The view has the instant.
pub struct Now<I>(PhantomData<fn() -> I>);

// ---------------------------------------------------------------- view shapes from the steps

/// The view of a component reached by steps that go through no optional
/// node: `&X` or `&mut X`.
pub struct Plain;
/// The view of a component reached through an at-most-one child: an
/// `Option`.
pub struct Opt;
/// The view of a component of every child by key ([`ByKey`]).
pub struct KeyedTag<K>(PhantomData<fn() -> K>);
/// The view of a component of every child but one by key
/// ([`ByKeyOthers`]).
pub struct OthersTag<K>(PhantomData<fn() -> K>);

/// The view shape the steps `Self` give, from the shape so far `W`; it
/// follows from the step types alone.
pub trait StepTag<W> {
    /// The shape after the steps.
    type Tag;
}
impl<W> StepTag<W> for Here {
    type Tag = W;
}
impl<W, B> StepTag<W> for ToChild<B, AtMost> {
    type Tag = Opt;
}
impl<W, B> StepTag<W> for ToChild<B, MyLink> {
    type Tag = W;
}
impl<B, K> StepTag<Plain> for ToChild<B, ByKey<K>> {
    type Tag = KeyedTag<K>;
}
impl<B, K> StepTag<Plain> for ToChild<B, ByKeyOthers<K>> {
    type Tag = OthersTag<K>;
}
impl<W, S: StepTag<W>, R: StepTag<S::Tag>> StepTag<W> for Then<S, R> {
    type Tag = R::Tag;
}
/// The view shape of the steps `S` from an anchor.
pub type TagOf<S> = <S as StepTag<Plain>>::Tag;

// ---------------------------------------------------------------- the chain up

/// A node on the way up: its kind `K` and its place `Pre`, the positions
/// of the children from it up to the root, nearest first.
pub struct On<K, Pre>(PhantomData<fn() -> (K, Pre)>);
/// The place segment of the linked node: the position `I` among the root's
/// children, reached by the link and not by the path.
pub struct LinkPos<I>(PhantomData<fn() -> I>);

/// The nodes on a typed path from a node of the kind `K` at `Pre`: the
/// node the path leads to first, the start last.
pub trait ChainOf<K, Pre> {
    /// The list of [`On`].
    type Chain;
}
impl<K, Pre> ChainOf<K, Pre> for End {
    type Chain = HCons<On<K, Pre>, HNil>;
}
impl<K: NodeKind, Pre, I, H, R> ChainOf<K, Pre> for Step<I, H, R>
where
    K::Children: ChildIndex<I>,
    R: ChainOf<KindAt<K::Children, I>, HCons<I, Pre>>,
    R::Chain: Add<HCons<On<K, Pre>, HNil>>,
{
    type Chain = <R::Chain as Add<HCons<On<K, Pre>, HNil>>>::Output;
}

/// The node of the kind `A` is at the position `I` of the chain.
pub trait FindKind<A, I> {
    /// Its place.
    type Pre;
}
impl<A, Pre, T> FindKind<A, I0> for HCons<On<A, Pre>, T> {
    type Pre = Pre;
}
impl<A, H, T: FindKind<A, I>, I> FindKind<A, There<I>> for HCons<H, T> {
    type Pre = T::Pre;
}

/// The node at the position `I` of the chain.
pub trait Nth<I> {
    /// Its kind.
    type Kind;
    /// Its place.
    type Pre;
}
impl<K, Pre, T> Nth<I0> for HCons<On<K, Pre>, T> {
    type Kind = K;
    type Pre = Pre;
}
impl<H, T: Nth<I>, I> Nth<There<I>> for HCons<H, T> {
    type Kind = T::Kind;
    type Pre = T::Pre;
}

/// The component `X` is in a node's data, a list of the parts' lists, at
/// the position `I` (which part, then where in its share).
pub trait HasIn<X, I> {
    /// The component.
    fn get_in(&self) -> &X;
    /// The component, exclusive.
    fn get_in_mut(&mut self) -> &mut X;
}
/// The position of a component in the first part's share, at `I` in it.
pub struct InHead<I>(PhantomData<fn() -> I>);
impl<X, H: Selector<X, I>, T, I> HasIn<X, InHead<I>> for HCons<H, T> {
    fn get_in(&self) -> &X {
        self.head.get()
    }
    fn get_in_mut(&mut self) -> &mut X {
        self.head.get_mut()
    }
}
impl<X, H, T: HasIn<X, J>, J> HasIn<X, There<J>> for HCons<H, T> {
    fn get_in(&self) -> &X {
        self.tail.get_in()
    }
    fn get_in_mut(&mut self) -> &mut X {
        self.tail.get_in_mut()
    }
}

/// The position on the chain of the node whose data has the component.
pub struct Found<I>(PhantomData<fn() -> I>);
/// The node on the chain whose data has `X` (for the parts `P`) is at `I`.
pub trait FindProvider<X, P, I> {
    /// Its kind.
    type Kind;
    /// Its place.
    type Pre;
}
impl<X, P, K: KindFor<P>, Pre, T, I> FindProvider<X, P, Found<I>> for HCons<On<K, Pre>, T>
where
    K::Data: HasIn<X, I>,
{
    type Kind = K;
    type Pre = Pre;
}
impl<X, P, H, T: FindProvider<X, P, I>, I> FindProvider<X, P, There<I>> for HCons<H, T> {
    type Kind = T::Kind;
    type Pre = T::Pre;
}

/// The position on the chain an [`Up`] anchor names.
pub trait Levels {
    /// The position, as a frunk index.
    type Index;
}
macro_rules! levels {
    ($($n:literal => $p:ty),*) => { $(impl Levels for Up<$n> { type Index = $p; })* };
}
levels!(
    0 => I0,
    1 => There<I0>,
    2 => There<There<I0>>,
    3 => There<There<There<I0>>>,
    4 => There<There<There<There<I0>>>>,
    5 => There<There<There<There<There<I0>>>>>
);

/// Where an anchor is on the chain `Chain`, for the parts `P`; `AI` is
/// inferred.
pub trait Anchor<Chain, P, AI> {
    /// The anchor's kind.
    type Kind;
    /// The anchor's place.
    type Pre;
}
impl<Chain: FindKind<A, AI>, P, A, AI> Anchor<Chain, P, AI> for Kind<A> {
    type Kind = A;
    type Pre = Chain::Pre;
}
impl<Chain, P, const N: usize> Anchor<Chain, P, ()> for Up<N>
where
    Up<N>: Levels,
    Chain: Nth<<Up<N> as Levels>::Index>,
{
    type Kind = <Chain as Nth<<Up<N> as Levels>::Index>>::Kind;
    type Pre = <Chain as Nth<<Up<N> as Levels>::Index>>::Pre;
}
impl<Chain: FindProvider<X, P, AI>, P, X, AI> Anchor<Chain, P, AI> for Provides<X> {
    type Kind = Chain::Kind;
    type Pre = Chain::Pre;
}

// ---------------------------------------------------------------- steps on the structure

/// The shape of what a [`ByKey`] step reaches: the whole store of the
/// storage kind `S`.
pub struct KeyedW<S>(PhantomData<fn() -> S>);
/// The shape of what a [`ByKeyOthers`] step reaches: the store of the
/// storage kind `S` but the node on the way up.
pub struct KeyedOthersW<S>(PhantomData<fn() -> S>);

/// A shape an at-most-one step may follow.
pub trait ToOpt {}
impl ToOpt for Plain {}
impl ToOpt for Opt {}

/// The steps `Self` from a node of the kind `K` at the place `Pre`, the
/// shape so far `W`; `SI` is inferred.
pub trait Steps<K, Pre, W, SI> {
    /// The kind reached.
    type Kind;
    /// The place reached.
    type Path;
    /// The shape of what is reached.
    type Wrap;
}
impl<K, Pre, W> Steps<K, Pre, W, ()> for Here {
    type Kind = K;
    type Path = Pre;
    type Wrap = W;
}
impl<K: NodeKind, Pre, W: ToOpt, B, I> Steps<K, Pre, W, I> for ToChild<B, AtMost>
where
    K::Children: ChildOfKind<B, I, Storage = AtMostOne>,
{
    type Kind = B;
    type Path = HCons<I, Pre>;
    type Wrap = Opt;
}
impl<K: NodeKind, Pre, B, I> Steps<K, Pre, Plain, I> for ToChild<B, MyLink>
where
    K::Children: ChildOfKind<B, I>,
{
    type Kind = B;
    type Path = HCons<LinkPos<I>, Pre>;
    type Wrap = Plain;
}
impl<K: NodeKind, Pre, B, Key, I> Steps<K, Pre, Plain, I> for ToChild<B, ByKey<Key>>
where
    K::Children: ChildOfKind<B, I, Storage: ChildStorage<Key = Key>>,
{
    type Kind = B;
    type Path = HCons<I, Pre>;
    type Wrap = KeyedW<<K::Children as ChildIndex<I>>::Storage>;
}
impl<K: NodeKind, Pre, B, Key, I> Steps<K, Pre, Plain, I> for ToChild<B, ByKeyOthers<Key>>
where
    K::Children: ChildOfKind<B, I, Storage: ChildStorage<Key = Key>>,
{
    type Kind = B;
    type Path = HCons<I, Pre>;
    type Wrap = KeyedOthersW<<K::Children as ChildIndex<I>>::Storage>;
}
impl<K, Pre, W, S, R, I, J> Steps<K, Pre, W, (I, J)> for Then<S, R>
where
    S: Steps<K, Pre, W, I>,
    R: Steps<S::Kind, S::Path, S::Wrap, J>,
{
    type Kind = R::Kind;
    type Path = R::Path;
    type Wrap = R::Wrap;
}

/// The kind of the anchor `A` on `Ch`.
pub type AnchorKind<A, Ch, P, AI> = <A as Anchor<Ch, P, AI>>::Kind;
/// The place of the anchor `A` on `Ch`.
pub type AnchorPre<A, Ch, P, AI> = <A as Anchor<Ch, P, AI>>::Pre;
/// The kind the steps `S` reach from the anchor `A`.
pub type StepsKind<A, S, Ch, P, AI, SI> =
    <S as Steps<AnchorKind<A, Ch, P, AI>, AnchorPre<A, Ch, P, AI>, Plain, SI>>::Kind;
/// The place the steps `S` reach from the anchor `A`.
pub type StepsPath<A, S, Ch, P, AI, SI> =
    <S as Steps<AnchorKind<A, Ch, P, AI>, AnchorPre<A, Ch, P, AI>, Plain, SI>>::Path;
/// The shape of what the steps `S` reach from the anchor `A`.
pub type StepsWrap<A, S, Ch, P, AI, SI> =
    <S as Steps<AnchorKind<A, Ch, P, AI>, AnchorPre<A, Ch, P, AI>, Plain, SI>>::Wrap;

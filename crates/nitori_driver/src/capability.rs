//! Capabilities: what a call takes besides components.
//!
//! A composed capability ([`Capability`], taken with [`Cap`]) is a list of
//! needs and a way to put their view together. Its needs are proved with
//! the call's, as if the call had written them, so an overlap between the
//! two does not compile.
//!
//! A contributed capability (taken with [`Contributed<F>`](Contributed))
//! is made of every part's piece for the family `F`. Each part writes
//! [`Contribute<F>`] for every family a call of the composition uses,
//! `Needs = HNil` and `Piece<'w> = ()` when it gives nothing. The pieces
//! are lined up in the order of the parts, as an `HList`
//! ([`PiecesOf`]); the family writes its rule on that list once. Two rules
//! are given here: dispatching a sum to the piece of its alternative
//! ([`DispatchSum`], each piece writing [`DispatchRun`]), and running every
//! piece in turn ([`EachPiece`], each piece writing [`EachRun`]). A piece
//! of `()` dispatches the empty sum and runs as nothing, written once.
//!
//! A call that uses a family writes the family's bound on the composition,
//! which the family wraps in a trait of its own:
//!
//! ```
//! use frunk_core::coproduct::{CNil, Coproduct};
//! use nitori_driver::{
//!     call::{Composition, PartsOf},
//!     capability::{DispatchRun, DispatchSum, PieceList},
//! };
//!
//! /// The family: every part's piece that takes the part's frames.
//! pub struct FrameDispatch;
//! /// A frame of the type `T`.
//! pub struct Frame<const T: u8>(pub u64);
//! /// The frames of a part, as the wire format defines them.
//! pub trait FrameSetPart {
//!     type Run;
//! }
//! /// The frames of a list of parts: one alternative per part.
//! pub trait FrameSet {
//!     type Sum;
//! }
//! impl FrameSet for frunk_core::hlist::HNil {
//!     type Sum = CNil;
//! }
//! impl<H: FrameSetPart, T: FrameSet> FrameSet for frunk_core::hlist::HCons<H, T> {
//!     type Sum = Coproduct<H::Run, T::Sum>;
//! }
//! /// What a call that dispatches frames asks of the composition.
//! pub trait DispatchesFrames:
//!     Composition<
//!         Parts: FrameSet
//!                    + for<'w> PieceList<
//!             FrameDispatch,
//!             Pieces<'w>: DispatchSum<FrameDispatch, <PartsOf<Self> as FrameSet>::Sum, (), u64>,
//!         >,
//!     >
//! {
//! }
//! ```
//!
//! A composed capability and a contributed one, lent at a root; the
//! second part gives nothing to the family:
//!
//! ```
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_driver::{
//! #     capability::{Cap, Capability, Contribute, Contributed, EachPiece, EachRun, PartNeeds},
//! #     lend::lend,
//! #     need::{At, Here, Kind, Mut, Ref, Up},
//! #     structure::{End, Node, NodeKind, Share},
//! # };
//! # struct Root;
//! # impl NodeKind for Root {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # struct Other;
//! # impl Share<Root> for Other {
//! #     type Data = HNil;
//! # }
//! # /// Counting: the count, exclusive.
//! # struct CountCap;
//! # impl Capability for CountCap {
//! #     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
//! #     type View<V> = V;
//! #     fn assemble<V>(needs: V) -> V {
//! #         needs
//! #     }
//! # }
//! # /// The family: every part's limit, read in turn.
//! # struct Limits;
//! # struct LimitPiece<'w>(&'w Limit);
//! # impl EachRun<Limits, u32> for LimitPiece<'_> {
//! #     fn each_run(&mut self, sum: &mut u32) {
//! #         *sum += self.0.0;
//! #     }
//! # }
//! # impl Contribute<Limits> for Part {
//! #     type Needs = HList![At<Kind<Root>, Here, Ref<Limit>>];
//! #     type Piece<'w> = LimitPiece<'w>;
//! #     fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> LimitPiece<'_> {
//! #         LimitPiece(needs.head)
//! #     }
//! # }
//! # let mut root: Node<Root, HList![Part, Other]> = Node::new(Default::default());
//! impl Contribute<Limits> for Other {
//!     type Needs = HNil;
//!     type Piece<'w> = ();
//!     fn assemble(_: <HNil as PartNeeds>::View<'_>) -> Self::Piece<'_> {}
//! }
//! type Needs = HList![Cap<CountCap>, Contributed<Limits>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, |view| {
//!     let hlist_pat![hlist_pat![count], mut limits] = view;
//!     limits.each_piece(&mut count.0);
//! })
//! .unwrap();
//! ```
//!
//! # What does not compile
//!
//! A capability and the call taking the same component:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_driver::{
//! #     capability::{Cap, Capability, Contribute, Contributed, EachPiece, EachRun, PartNeeds},
//! #     lend::lend,
//! #     need::{At, Here, Kind, Mut, Ref, Up},
//! #     structure::{End, Node, NodeKind, Share},
//! # };
//! # struct Root;
//! # impl NodeKind for Root {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # struct Other;
//! # impl Share<Root> for Other {
//! #     type Data = HNil;
//! # }
//! # /// Counting: the count, exclusive.
//! # struct CountCap;
//! # impl Capability for CountCap {
//! #     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
//! #     type View<V> = V;
//! #     fn assemble<V>(needs: V) -> V {
//! #         needs
//! #     }
//! # }
//! # /// The family: every part's limit, read in turn.
//! # struct Limits;
//! # struct LimitPiece<'w>(&'w Limit);
//! # impl EachRun<Limits, u32> for LimitPiece<'_> {
//! #     fn each_run(&mut self, sum: &mut u32) {
//! #         *sum += self.0.0;
//! #     }
//! # }
//! # impl Contribute<Limits> for Part {
//! #     type Needs = HList![At<Kind<Root>, Here, Ref<Limit>>];
//! #     type Piece<'w> = LimitPiece<'w>;
//! #     fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> LimitPiece<'_> {
//! #         LimitPiece(needs.head)
//! #     }
//! # }
//! # let mut root: Node<Root, HList![Part, Other]> = Node::new(Default::default());
//! impl Contribute<Limits> for Other {
//!     type Needs = HNil;
//!     type Piece<'w> = ();
//!     fn assemble(_: <HNil as PartNeeds>::View<'_>) -> Self::Piece<'_> {}
//! }
//! type Needs = HList![Cap<CountCap>, At<Kind<Root>, Here, Ref<Count>>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, |_| ()).unwrap();
//! ```
//!
//! A part's piece and the call taking the same component exclusive:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_driver::{
//! #     capability::{Cap, Capability, Contribute, Contributed, EachPiece, EachRun, PartNeeds},
//! #     lend::lend,
//! #     need::{At, Here, Kind, Mut, Ref, Up},
//! #     structure::{End, Node, NodeKind, Share},
//! # };
//! # struct Root;
//! # impl NodeKind for Root {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # struct Other;
//! # impl Share<Root> for Other {
//! #     type Data = HNil;
//! # }
//! # /// Counting: the count, exclusive.
//! # struct CountCap;
//! # impl Capability for CountCap {
//! #     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
//! #     type View<V> = V;
//! #     fn assemble<V>(needs: V) -> V {
//! #         needs
//! #     }
//! # }
//! # /// The family: every part's limit, read in turn.
//! # struct Limits;
//! # struct LimitPiece<'w>(&'w Limit);
//! # impl EachRun<Limits, u32> for LimitPiece<'_> {
//! #     fn each_run(&mut self, sum: &mut u32) {
//! #         *sum += self.0.0;
//! #     }
//! # }
//! # impl Contribute<Limits> for Part {
//! #     type Needs = HList![At<Kind<Root>, Here, Ref<Limit>>];
//! #     type Piece<'w> = LimitPiece<'w>;
//! #     fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> LimitPiece<'_> {
//! #         LimitPiece(needs.head)
//! #     }
//! # }
//! # let mut root: Node<Root, HList![Part, Other]> = Node::new(Default::default());
//! impl Contribute<Limits> for Other {
//!     type Needs = HNil;
//!     type Piece<'w> = ();
//!     fn assemble(_: <HNil as PartNeeds>::View<'_>) -> Self::Piece<'_> {}
//! }
//! type Needs = HList![At<Kind<Root>, Here, Mut<Limit>>, Contributed<Limits>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, |_| ()).unwrap();
//! ```
//!
//! A part that does not say what it contributes to a family a need uses:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_driver::{
//! #     capability::{Cap, Capability, Contribute, Contributed, EachPiece, EachRun, PartNeeds},
//! #     lend::lend,
//! #     need::{At, Here, Kind, Mut, Ref, Up},
//! #     structure::{End, Node, NodeKind, Share},
//! # };
//! # struct Root;
//! # impl NodeKind for Root {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # struct Other;
//! # impl Share<Root> for Other {
//! #     type Data = HNil;
//! # }
//! # /// Counting: the count, exclusive.
//! # struct CountCap;
//! # impl Capability for CountCap {
//! #     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
//! #     type View<V> = V;
//! #     fn assemble<V>(needs: V) -> V {
//! #         needs
//! #     }
//! # }
//! # /// The family: every part's limit, read in turn.
//! # struct Limits;
//! # struct LimitPiece<'w>(&'w Limit);
//! # impl EachRun<Limits, u32> for LimitPiece<'_> {
//! #     fn each_run(&mut self, sum: &mut u32) {
//! #         *sum += self.0.0;
//! #     }
//! # }
//! # impl Contribute<Limits> for Part {
//! #     type Needs = HList![At<Kind<Root>, Here, Ref<Limit>>];
//! #     type Piece<'w> = LimitPiece<'w>;
//! #     fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> LimitPiece<'_> {
//! #         LimitPiece(needs.head)
//! #     }
//! # }
//! # let mut root: Node<Root, HList![Part, Other]> = Node::new(Default::default());
//! type Needs = HList![Contributed<Limits>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, |_| ()).unwrap();
//! ```
use core::marker::PhantomData;

use frunk_core::{
    coproduct::{CNil, Coproduct},
    hlist::{HCons, HNil},
};

use crate::{
    lend::{
        Build, Flat, KeyedList, KeyedPositions, KeyedShape, Locate, NeedList, Pass1, Pass2, Places,
    },
    need::{At, Mut, Opt, Plain, Ref, StepTag, TagOf},
};

// ---------------------------------------------------------------- composed capabilities

/// Take the composed capability `C`: its needs, proved with the call's,
/// put together into its view.
pub struct Cap<C>(PhantomData<fn() -> C>);

/// A composed capability: a list of needs and the view made of their view.
pub trait Capability: 'static {
    /// The needs; they may take other capabilities.
    type Needs;
    /// The view, made of the needs' view `V`.
    type View<V>;
    /// Put the view together.
    fn assemble<V>(needs: V) -> Self::View<V>;
}

impl<Cp: Capability<Needs: Flat>> Flat for Cap<Cp> {
    type Keyed = <Cp::Needs as Flat>::Keyed;
}
impl<P, L: KeyedShape, Cp: Capability<Needs: NeedList<P, L>>> NeedList<P, L> for Cap<Cp> {
    type View<'w, KP: KeyedList<P, L>> = Cp::View<<Cp::Needs as NeedList<P, L>>::View<'w, KP>>;
    type Rest = <Cp::Needs as NeedList<P, L>>::Rest;
    type RestPos<KP: KeyedList<P, L>> = <Cp::Needs as NeedList<P, L>>::RestPos<KP>;
}
impl<'a, Cx, U, Cp: Capability, I> Pass1<'a, Cx, U, I> for Cap<Cp>
where
    Cp::Needs: Pass1<'a, Cx, U, I>,
{
    type Raw = <Cp::Needs as Pass1<'a, Cx, U, I>>::Raw;
    type Rem = <Cp::Needs as Pass1<'a, Cx, U, I>>::Rem;
    fn take(u: U) -> (Self::Raw, Self::Rem) {
        Cp::Needs::take(u)
    }
}
impl<'a, Cx, Fr, Cp: Capability, I> Pass2<'a, Cx, Fr, I> for Cap<Cp>
where
    Cp::Needs: Pass2<'a, Cx, Fr, I>,
{
    type Raw = <Cp::Needs as Pass2<'a, Cx, Fr, I>>::Raw;
    fn get(f: &Fr) -> Self::Raw {
        Cp::Needs::get(f)
    }
}
impl<Cx, U, Cp: Capability, I> Locate<Cx, U, I> for Cap<Cp> where Cp::Needs: Locate<Cx, U, I> {}
impl<Cx, U, Cp: Capability, I> Places<Cx, U, I> for Cap<Cp>
where
    Cp::Needs: Places<Cx, U, I>,
{
    type Out = <Cp::Needs as Places<Cx, U, I>>::Out;
}
impl<Cx, Cp: Capability, I> KeyedPositions<Cx, I> for Cap<Cp>
where
    Cp::Needs: KeyedPositions<Cx, I>,
{
    type KP = <Cp::Needs as KeyedPositions<Cx, I>>::KP;
}
impl<'a, P, L, KP, Cp, R1, R2> Build<'a, P, L, KP, R1, R2> for Cap<Cp>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
    Cp: Capability<Needs: Build<'a, P, L, KP, R1, R2>>,
{
    fn view(r1: R1, r2: R2) -> Self::View<'a, KP> {
        Cp::assemble(<Cp::Needs as Build<'a, P, L, KP, R1, R2>>::view(r1, r2))
    }
}

// ---------------------------------------------------------------- contributed capabilities

/// Take the contributed capability of the family `F`: every part's piece,
/// in part order.
pub struct Contributed<F>(PhantomData<fn() -> F>);

/// What the part `Self` contributes to the family `F`: needs, and the
/// piece put together from their view.
///
/// Every part of a composition writes it for every family a call of the
/// composition uses; a part that gives nothing writes `Needs = HNil` and
/// `Piece<'w> = ()`. The needs are components and composed capabilities;
/// they are not by key.
#[diagnostic::on_unimplemented(
    message = "the part `{Self}` does not say what it contributes to the family `{F}`",
    note = "every part writes `Contribute<{F}>` when a call of the composition uses the family; `Needs = HNil` and `Piece<'w> = ()` for nothing"
)]
pub trait Contribute<F> {
    /// The needs, an `HList`.
    type Needs: PartNeeds;
    /// The piece.
    type Piece<'w>;
    /// Put the piece together.
    fn assemble(needs: <Self::Needs as PartNeeds>::View<'_>) -> Self::Piece<'_>;
}
/// The pieces of a list of parts for the family `F`, in part order.
pub trait PieceList<F> {
    /// Every part's needs, one list per part.
    type Needs;
    /// Every part's piece.
    type Pieces<'w>;
}
impl<F> PieceList<F> for HNil {
    type Needs = HNil;
    type Pieces<'w> = HNil;
}
impl<F, Pt: Contribute<F>, T: PieceList<F>> PieceList<F> for HCons<Pt, T> {
    type Needs = HCons<Pt::Needs, T::Needs>;
    type Pieces<'w> = HCons<Pt::Piece<'w>, T::Pieces<'w>>;
}
/// The pieces of the parts `P` for the family `F`.
pub type PiecesOf<'w, P, F> = <P as PieceList<F>>::Pieces<'w>;
/// The needs of the parts `P` for the family `F`, one list per part.
pub type NeedsOf<P, F> = <P as PieceList<F>>::Needs;

/// The view a step shape makes of a component in a part's needs.
pub trait TagView<X: 'static> {
    /// Exclusive.
    type Mut<'w>;
    /// Shared.
    type Ref<'w>;
}
impl<X: 'static> TagView<X> for Plain {
    type Mut<'w> = &'w mut X;
    type Ref<'w> = &'w X;
}
impl<X: 'static> TagView<X> for Opt {
    type Mut<'w> = Option<&'w mut X>;
    type Ref<'w> = Option<&'w X>;
}

/// The view of a part's needs; it does not depend on the composition.
pub trait PartNeeds {
    /// The view.
    type View<'w>;
}
impl PartNeeds for HNil {
    type View<'w> = HNil;
}
impl<N: PartNeeds, T: PartNeeds> PartNeeds for HCons<N, T> {
    type View<'w> = HCons<N::View<'w>, T::View<'w>>;
}
impl<A, S: StepTag<Plain, Tag: TagView<X>>, X: 'static> PartNeeds for At<A, S, Mut<X>> {
    type View<'w> = <TagOf<S> as TagView<X>>::Mut<'w>;
}
impl<A, S: StepTag<Plain, Tag: TagView<X>>, X: 'static> PartNeeds for At<A, S, Ref<X>> {
    type View<'w> = <TagOf<S> as TagView<X>>::Ref<'w>;
}
impl<Cp: Capability<Needs: PartNeeds>> PartNeeds for Cap<Cp> {
    type View<'w> = Cp::View<<Cp::Needs as PartNeeds>::View<'w>>;
}

/// A part's view from the two passes' results.
pub trait PartBuild<'a, R1, R2>: PartNeeds {
    /// Build the view.
    fn view(r1: R1, r2: R2) -> Self::View<'a>;
}
impl<'a> PartBuild<'a, HNil, HNil> for HNil {
    fn view(_: HNil, _: HNil) -> HNil {
        HNil
    }
}
impl<'a, N, T, A1, A2, B1, B2> PartBuild<'a, HCons<A1, B1>, HCons<A2, B2>> for HCons<N, T>
where
    N: PartBuild<'a, A1, A2>,
    T: PartBuild<'a, B1, B2>,
{
    fn view(r1: HCons<A1, B1>, r2: HCons<A2, B2>) -> Self::View<'a> {
        HCons {
            head: N::view(r1.head, r2.head),
            tail: T::view(r1.tail, r2.tail),
        }
    }
}
/// The view of an exclusive component from pass 1's result.
pub trait TagViewBuild<'a, X: 'static, R1>: TagView<X> {
    /// Build the view.
    fn view(r1: R1) -> Self::Mut<'a>;
}
impl<'a, X: 'static> TagViewBuild<'a, X, &'a mut X> for Plain {
    fn view(r1: &'a mut X) -> &'a mut X {
        r1
    }
}
impl<'a, X: 'static> TagViewBuild<'a, X, Option<&'a mut X>> for Opt {
    fn view(r1: Option<&'a mut X>) -> Option<&'a mut X> {
        r1
    }
}
/// The view of a shared component from pass 2's result.
pub trait TagViewRefBuild<'a, X: 'static, R2>: TagView<X> {
    /// Build the view.
    fn view(r2: R2) -> Self::Ref<'a>;
}
impl<'a, X: 'static> TagViewRefBuild<'a, X, &'a X> for Plain {
    fn view(r2: &'a X) -> &'a X {
        r2
    }
}
impl<'a, X: 'static> TagViewRefBuild<'a, X, Option<&'a X>> for Opt {
    fn view(r2: Option<&'a X>) -> Option<&'a X> {
        r2
    }
}
impl<'a, A, S, X: 'static, R1> PartBuild<'a, R1, ()> for At<A, S, Mut<X>>
where
    S: StepTag<Plain, Tag: TagViewBuild<'a, X, R1>>,
{
    fn view(r1: R1, _: ()) -> Self::View<'a> {
        <TagOf<S> as TagViewBuild<'a, X, R1>>::view(r1)
    }
}
impl<'a, A, S, X: 'static, R2> PartBuild<'a, (), R2> for At<A, S, Ref<X>>
where
    S: StepTag<Plain, Tag: TagViewRefBuild<'a, X, R2>>,
{
    fn view(_: (), r2: R2) -> Self::View<'a> {
        <TagOf<S> as TagViewRefBuild<'a, X, R2>>::view(r2)
    }
}
impl<'a, Cp, R1, R2> PartBuild<'a, R1, R2> for Cap<Cp>
where
    Cp: Capability<Needs: PartBuild<'a, R1, R2>>,
{
    fn view(r1: R1, r2: R2) -> Self::View<'a> {
        Cp::assemble(<Cp::Needs as PartBuild<'a, R1, R2>>::view(r1, r2))
    }
}

/// Every part's piece for `F`, in part order, from the two passes'
/// results.
pub trait AssemblePieces<'a, F, R1, R2>: PieceList<F> {
    /// Put every piece together.
    fn assemble(r1: R1, r2: R2) -> Self::Pieces<'a>;
}
impl<'a, F> AssemblePieces<'a, F, HNil, HNil> for HNil {
    fn assemble(_: HNil, _: HNil) -> HNil {
        HNil
    }
}
impl<'a, F, Pt, T, A1, A2, B1, B2> AssemblePieces<'a, F, HCons<A1, B1>, HCons<A2, B2>>
    for HCons<Pt, T>
where
    Pt: Contribute<F>,
    Pt::Needs: PartBuild<'a, A1, A2>,
    T: AssemblePieces<'a, F, B1, B2>,
{
    fn assemble(r1: HCons<A1, B1>, r2: HCons<A2, B2>) -> Self::Pieces<'a> {
        HCons {
            head: Pt::assemble(<Pt::Needs as PartBuild<'a, A1, A2>>::view(r1.head, r2.head)),
            tail: T::assemble(r1.tail, r2.tail),
        }
    }
}

/// A contribution's needs are not by key: it has no keyed needs.
impl<F> Flat for Contributed<F> {
    type Keyed = HNil;
}
impl<P: PieceList<F>, L: KeyedShape, F> NeedList<P, L> for Contributed<F> {
    type View<'w, KP: KeyedList<P, L>> = PiecesOf<'w, P, F>;
    type Rest = L;
    type RestPos<KP: KeyedList<P, L>> = KP;
}
impl<'a, Ch, P, U, F, I> Pass1<'a, crate::lend::Cx<Ch, P>, U, I> for Contributed<F>
where
    P: PieceList<F>,
    NeedsOf<P, F>: Pass1<'a, crate::lend::Cx<Ch, P>, U, I>,
{
    type Raw = <NeedsOf<P, F> as Pass1<'a, crate::lend::Cx<Ch, P>, U, I>>::Raw;
    type Rem = <NeedsOf<P, F> as Pass1<'a, crate::lend::Cx<Ch, P>, U, I>>::Rem;
    fn take(u: U) -> (Self::Raw, Self::Rem) {
        <NeedsOf<P, F>>::take(u)
    }
}
impl<'a, Ch, P, Fr, F, I> Pass2<'a, crate::lend::Cx<Ch, P>, Fr, I> for Contributed<F>
where
    P: PieceList<F>,
    NeedsOf<P, F>: Pass2<'a, crate::lend::Cx<Ch, P>, Fr, I>,
{
    type Raw = <NeedsOf<P, F> as Pass2<'a, crate::lend::Cx<Ch, P>, Fr, I>>::Raw;
    fn get(f: &Fr) -> Self::Raw {
        <NeedsOf<P, F>>::get(f)
    }
}
impl<Ch, P, U, F, I> Locate<crate::lend::Cx<Ch, P>, U, I> for Contributed<F>
where
    P: PieceList<F>,
    NeedsOf<P, F>: Locate<crate::lend::Cx<Ch, P>, U, I>,
{
}
impl<Ch, P, U, F, I> Places<crate::lend::Cx<Ch, P>, U, I> for Contributed<F>
where
    P: PieceList<F>,
    NeedsOf<P, F>: Places<crate::lend::Cx<Ch, P>, U, I>,
{
    type Out = <NeedsOf<P, F> as Places<crate::lend::Cx<Ch, P>, U, I>>::Out;
}
impl<Cx, F> KeyedPositions<Cx, ()> for Contributed<F> {
    type KP = HNil;
}
impl<'a, P, L, KP, F, R1, R2> Build<'a, P, L, KP, R1, R2> for Contributed<F>
where
    L: KeyedShape,
    KP: KeyedList<P, L>,
    P: AssemblePieces<'a, F, R1, R2>,
{
    fn view(r1: R1, r2: R2) -> PiecesOf<'a, P, F> {
        <P as AssemblePieces<'a, F, R1, R2>>::assemble(r1, r2)
    }
}

// ---------------------------------------------------------------- the two rules

/// A piece of the family `F` dispatches `R`, its part's alternative of a
/// sum, with the argument `A`, giving `O`.
///
/// The piece does not name its part's type for `R`; the sum is defined
/// once elsewhere (for frames, by the wire format), and [`DispatchSum`]
/// lines it up with the pieces by position.
pub trait DispatchRun<F, R, A, O> {
    /// Dispatch the item.
    fn dispatch_run(&mut self, item: R, arg: A) -> O;
}
/// The piece of a part that gives nothing dispatches the empty sum.
impl<F, A, O> DispatchRun<F, CNil, A, O> for () {
    fn dispatch_run(&mut self, item: CNil, _: A) -> O {
        match item {}
    }
}
/// The pieces of the family `F` dispatch the sum `R`: the head of the sum
/// to the first piece, the rest to the others.
pub trait DispatchSum<F, R, A, O> {
    /// Dispatch the item to the piece of its alternative.
    fn dispatch_sum(&mut self, item: R, arg: A) -> O;
}
impl<F, A, O> DispatchSum<F, CNil, A, O> for HNil {
    fn dispatch_sum(&mut self, item: CNil, _: A) -> O {
        match item {}
    }
}
impl<F, A, O, H, T, RH, RT> DispatchSum<F, Coproduct<RH, RT>, A, O> for HCons<H, T>
where
    H: DispatchRun<F, RH, A, O>,
    T: DispatchSum<F, RT, A, O>,
{
    fn dispatch_sum(&mut self, item: Coproduct<RH, RT>, arg: A) -> O {
        match item {
            Coproduct::Inl(h) => self.head.dispatch_run(h, arg),
            Coproduct::Inr(t) => self.tail.dispatch_sum(t, arg),
        }
    }
}

/// A piece of the family `F` run in turn with the argument `A`.
pub trait EachRun<F, A> {
    /// Run the piece.
    fn each_run(&mut self, arg: &mut A);
}
/// The piece of a part that gives nothing runs as nothing.
impl<F, A> EachRun<F, A> for () {
    fn each_run(&mut self, _: &mut A) {}
}
/// The pieces of the family `F` run every piece in turn, in part order.
pub trait EachPiece<F, A> {
    /// Run every piece.
    fn each_piece(&mut self, arg: &mut A);
}
impl<F, A> EachPiece<F, A> for HNil {
    fn each_piece(&mut self, _: &mut A) {}
}
impl<F, A, H: EachRun<F, A>, T: EachPiece<F, A>> EachPiece<F, A> for HCons<H, T> {
    fn each_piece(&mut self, arg: &mut A) {
        self.head.each_run(arg);
        self.tail.each_piece(arg);
    }
}

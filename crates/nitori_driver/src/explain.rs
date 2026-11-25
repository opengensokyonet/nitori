//! Proving one lending on its own: [`explain!`](crate::explain!).

/// The build needs `$n` of a component at the kind `$k` under the
/// composition `$q`, proved on their own as where `$q` is composed; what
/// [`explain!`](crate::explain!) expands to for a component, beside its
/// own needs.
#[doc(hidden)]
#[macro_export]
macro_rules! __explain_build {
    ($q:ty, $k:ty, $n:ty) => {
        const _: () = {
            let _ = $crate::__private::assert_build_lends::<$q, $k, $n, _>;
        };
    };
}
/// One lending proved on its own: the needs `$n` at the kind `$k` under
/// the composition `$q`; what [`explain!`](crate::explain!) expands to.
#[doc(hidden)]
#[macro_export]
macro_rules! __explain_needs {
    ($q:ty, $k:ty, $n:ty) => {
        const _: () = {
            let _ = $crate::__private::assert_lends_in::<
                $crate::call::RootOf<$q>,
                $crate::call::PartsOf<$q>,
                $crate::call::ContextOf<$q>,
                $k,
                $n,
                _,
                _,
            >;
        };
    };
}
/// Prove one lending on its own, at the kind `K` under the composition
/// `Q`, where an error says why: the needs of a call, those of a
/// component's operations and of its build, or a list of needs written
/// out.
///
/// ```text
/// explain!(Q, K, call C);            // the call kind C: <C as CallKind<Q, K>>::Needs
/// explain!(Q, K, component C);       // the component C: itself, exclusive, then its Needs; and its BuildNeeds
/// explain!(Q, K, component C of O);  // the same, C a component of the part O
/// explain!(Q, K, needs![...]);       // a list of needs
/// ```
///
/// [`composition!`](crate::composition!) proves every lending of the
/// structure at once, and when one fails, its error often names only the
/// lending (`` the trait bound `…: Lends<…>` is not satisfied ``), not the
/// need that fails or why. `explain!` proves that lending alone, with the
/// same checks, and its error starts with the reason, as a need's does
/// (for example `` taking `X` on the way up: … ``).
///
/// What it does:
///
/// - It expands to one item, `const _: () = …;`, written anywhere in a
///   module where `Q` is in scope; it makes nothing and runs nothing.
///   `Q` is a composition ([`Composition`](crate::call::Composition)), so
///   its `composition!` must be there too; while that one does not
///   compile, its errors are reported beside `explain!`'s.
/// - The root, the parts and the context lent with are `Q`'s
///   ([`RootOf`](crate::call::RootOf), [`PartsOf`](crate::call::PartsOf),
///   [`ContextOf`](crate::call::ContextOf)); the positions of the needs
///   are inferred, as where `Q` is composed.
/// - `call C` proves `<C as CallKind<Q, K>>::Needs`
///   ([`CallKind`](crate::call::CallKind)): the needs only, not that the
///   kind's `Op` runs on its lending.
/// - `component C` proves what the component's `wake` and `end` are
///   lent: `C` itself, exclusive, then its
///   [`Needs`](crate::component::Component::Needs), at `K`; and, as a
///   lending of its own, what its `build` is lent: its
///   [`BuildNeeds`](crate::component::Component::BuildNeeds), from the
///   nodes above a new node of `K`, or none when `K` is the root kind, as
///   where `Q` is composed. The part whose share `C` is in is inferred when `C` is
///   a component of one part only at `K`; a component of any part (`impl<O,
///   L, K> Component<O, L, K>`) leaves it open ("type annotations
///   needed"), and is written `component C of O`. In that form `C` is a
///   path, with or without generic arguments (`Held<Rtt>`), since a type
///   in a macro cannot be followed by `of`.
/// - `needs![...]`, or any type, is proved as the list of needs it is.
///   A kind's handler ([`HandleRequest`](crate::request::HandleRequest))
///   has no form of its own: its needs are written out, as
///   `explain!(Q, K, <H as HandleRequest<K, PriorityOf<Q>>>::Needs)`
///   ([`PriorityOf`](crate::call::PriorityOf)), and proved at the nodes of
///   `K`, where an edit handle lends them.
///
/// # Examples
///
/// ```
/// #![feature(impl_trait_in_assoc_type)]
/// use nitori_call::{LentAccess, call};
/// use nitori_driver::{
///     __private::calls::{Count, Item, Root, StillClock, Total},
///     call::{CallFamily, CallKind, Composition, LoanNeeds},
///     component::{ArgsOf, Component, Part, PartList},
///     composition, explain,
///     frunk_core::{HList, hlist::HNil, hlist_pat},
///     id::Numbered,
///     need::{Mut, Pos, Ref, View},
///     needs,
///     structure::{NodeKind, Share},
/// };
///
/// /// On the root: counts into the total.
/// impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Counting {
///     type Needs = HList![Mut<Total>];
///     type Op = Self;
///     fn start(_: &mut K::Start) -> Self {
///         Counting::new()
///     }
/// }
/// #[call]
/// async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
///     host.with(|mut view| {
///         let hlist_pat![total] = view.needs();
///         total.0 += 1;
///     });
/// }
/// /// An item's tally, a component of the part `Counter` only.
/// #[derive(Numbered)]
/// struct Tally;
/// impl<L: PartList<L>> Component<Counter, L, Item> for Tally {
///     type Needs = HList![Ref<Total>];
///     fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
///         Tally
///     }
/// }
/// #[derive(Numbered)]
/// struct Counter;
/// impl<L> Part<L> for Counter {
///     type Args = ();
///     type Shares = HList![
///         Share<Root, HList![Total], HList![Counting]>,
///         Share<Item, HList![Count, Tally]>,
///     ];
/// }
/// struct Q;
/// composition!(impl for Q: Root, HList![Counter], StillClock, u8);
///
/// explain!(Q, Root, call Counting);
/// explain!(Q, Item, component Tally);
/// explain!(Q, Item, component Count of Counter);
/// explain!(Q, Item, needs![Mut<Count>, Ref<Total>]);
/// ```
///
/// The other forms: a component kept in
/// [`Held`](crate::component::Held), named with its generic argument and
/// its part; a component named by a path; and a component whose build
/// takes the total from the root above its node:
///
/// ```
/// # #![feature(impl_trait_in_assoc_type)]
/// # use nitori_call::{LentAccess, call};
/// # use nitori_driver::{
/// #     __private::calls::{Count, Item, Root, StillClock, Total},
/// #     call::{CallFamily, CallKind, Composition, LoanNeeds},
/// #     component::{ArgsOf, Component, Held, Part, PartList},
/// #     composition, explain,
/// #     frunk_core::{HList, hlist::HNil, hlist_pat},
/// #     id::Numbered,
/// #     need::{Mut, Pos, Ref, View},
/// #     needs,
/// #     structure::{NodeKind, Share},
/// # };
/// #
/// # /// On the root: counts into the total.
/// # impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Counting {
/// #     type Needs = HList![Mut<Total>];
/// #     type Op = Self;
/// #     fn start(_: &mut K::Start) -> Self {
/// #         Counting::new()
/// #     }
/// # }
/// # #[call]
/// # async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
/// #     host.with(|mut view| {
/// #         let hlist_pat![total] = view.needs();
/// #         total.0 += 1;
/// #     });
/// # }
/// # /// An item's tally, a component of the part `Counter` only.
/// # #[derive(Numbered)]
/// # struct Tally;
/// # impl<L: PartList<L>> Component<Counter, L, Item> for Tally {
/// #     type Needs = HList![Ref<Total>];
/// #     fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
/// #         Tally
/// #     }
/// # }
/// /// A host's estimator.
/// #[derive(Numbered, Default)]
/// struct Rtt;
/// impl<X: Numbered + Default + 'static, L: PartList<L>> Component<Counter, L, Item> for Held<X> {
///     fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
///         Held(X::default())
///     }
/// }
/// mod inner {
///     use super::*;
///     /// What an item started from: the total when it was added.
///     #[derive(Numbered)]
///     pub struct Seen(pub u32);
///     impl<L: PartList<L>> Component<Counter, L, Item> for Seen {
///         type BuildNeeds = HList![Ref<Total>];
///         fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, view: View<'_, Self::BuildNeeds, P>) -> Self {
///             let hlist_pat![total] = view;
///             Seen(total.0)
///         }
///     }
/// }
/// #[derive(Numbered)]
/// struct Counter;
/// impl<L> Part<L> for Counter {
///     type Args = ();
///     type Shares = HList![
///         Share<Root, HList![Total], HList![Counting]>,
///         Share<Item, HList![Count, Tally, Held<Rtt>, inner::Seen]>,
///     ];
/// }
/// struct Q;
/// composition!(impl for Q: Root, HList![Counter], StillClock, u8);
///
/// explain!(Q, Item, component Held<Rtt> of Counter);
/// explain!(Q, Item, component inner::Seen);
/// explain!(Q, Item, component crate::inner::Seen of Counter);
/// explain!(Q, Root, component Total of Counter);
/// # fn main() {}
/// ```
///
/// A call that takes a component no node of the structure has: where it
/// is composed, the error names the lending, `` the trait bound
/// `HCons<Mut<Total>, _>: Lends<Root, _, Root, IsSelf, _, _>` is not
/// satisfied ``, and not the need:
///
/// ```compile_fail,E0277
/// #![feature(impl_trait_in_assoc_type)]
/// use nitori_call::{LentAccess, call};
/// use nitori_driver::{
///     __private::calls::{Count, Item, Root, StillClock, Total},
///     call::{CallFamily, CallKind, Composition, LoanNeeds},
///     component::{ArgsOf, Component, Part, PartList},
///     composition, explain,
///     frunk_core::{HList, hlist::HNil, hlist_pat},
///     id::Numbered,
///     need::{Mut, Pos, Ref, View},
///     needs,
///     structure::{NodeKind, Share},
/// };
///
/// /// On the root: counts into the total.
/// impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Counting {
///     type Needs = HList![Mut<Total>, Ref<Orphan>];
///     type Op = Self;
///     fn start(_: &mut K::Start) -> Self {
///         Counting::new()
///     }
/// }
/// #[call]
/// async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
///     host.with(|mut view| {
///         let hlist_pat![total, _] = view.needs();
///         total.0 += 1;
///     });
/// }
/// /// An item's tally, a component of the part `Counter` only.
/// #[derive(Numbered)]
/// struct Tally;
/// impl<L: PartList<L>> Component<Counter, L, Item> for Tally {
///     type Needs = HList![Ref<Total>];
///     fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
///         Tally
///     }
/// }
/// /// Kept by no node of the structure.
/// #[derive(Numbered)]
/// struct Orphan;
/// struct Counter;
/// impl<L> Part<L> for Counter {
///     type Args = ();
///     type Shares = HList![
///         Share<Root, HList![Total], HList![Counting]>,
///         Share<Item, HList![Count, Tally]>,
///     ];
/// }
/// struct Q;
/// composition!(impl for Q: Root, HList![Counter], StillClock, u8);
/// ```
///
/// The same call explained: the error starts with `` taking `Orphan` on
/// the way up: … of the nodes from the lend point up to the root have it,
/// and exactly one must `` (none of them has it):
///
/// ```compile_fail,E0277
/// # #![feature(impl_trait_in_assoc_type)]
/// # use nitori_call::{LentAccess, call};
/// # use nitori_driver::{
/// #     __private::calls::{Count, Item, Root, StillClock, Total},
/// #     call::{CallFamily, CallKind, Composition, LoanNeeds},
/// #     component::{ArgsOf, Component, Part, PartList},
/// #     composition, explain,
/// #     frunk_core::{HList, hlist::HNil, hlist_pat},
/// #     id::Numbered,
/// #     need::{Mut, Pos, Ref, View},
/// #     needs,
/// #     structure::{NodeKind, Share},
/// # };
/// #
/// # /// On the root: counts into the total.
/// # impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Counting {
/// #     type Needs = HList![Mut<Total>, Ref<Orphan>];
/// #     type Op = Self;
/// #     fn start(_: &mut K::Start) -> Self {
/// #         Counting::new()
/// #     }
/// # }
/// # #[call]
/// # async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
/// #     host.with(|mut view| {
/// #         let hlist_pat![total, _] = view.needs();
/// #         total.0 += 1;
/// #     });
/// # }
/// # /// An item's tally, a component of the part `Counter` only.
/// # #[derive(Numbered)]
/// # struct Tally;
/// # impl<L: PartList<L>> Component<Counter, L, Item> for Tally {
/// #     type Needs = HList![Ref<Total>];
/// #     fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
/// #         Tally
/// #     }
/// # }
/// # /// Kept by no node of the structure.
/// # #[derive(Numbered)]
/// # struct Orphan;
/// # struct Counter;
/// # impl<L> Part<L> for Counter {
/// #     type Args = ();
/// #     type Shares = HList![
/// #         Share<Root, HList![Total], HList![Counting]>,
/// #         Share<Item, HList![Count, Tally]>,
/// #     ];
/// # }
/// # struct Q;
/// # composition!(impl for Q: Root, HList![Counter], StillClock, u8);
/// explain!(Q, Root, call Counting);
/// ```
///
/// A composition that compiles, and a lending explained on its own that
/// does not: a list of needs taking a component no node of the structure
/// has. The error is `explain!`'s alone, and starts with the reason,
/// `` taking `Orphan` on the way up: … of the nodes from the lend point up
/// to the root have it, and exactly one must ``:
///
/// ```compile_fail,E0277
/// # #![feature(impl_trait_in_assoc_type)]
/// # use nitori_call::{LentAccess, call};
/// # use nitori_driver::{
/// #     __private::calls::{Count, Item, Root, StillClock, Total},
/// #     call::{CallFamily, CallKind, Composition, LoanNeeds},
/// #     component::{ArgsOf, Component, Part, PartList},
/// #     composition, explain,
/// #     frunk_core::{HList, hlist::HNil, hlist_pat},
/// #     id::Numbered,
/// #     need::{Mut, Pos, Ref, View},
/// #     needs,
/// #     structure::{NodeKind, Share},
/// # };
/// #
/// # /// On the root: counts into the total.
/// # impl<Q: Composition, K: NodeKind> CallKind<Q, K> for Counting {
/// #     type Needs = HList![Mut<Total>];
/// #     type Op = Self;
/// #     fn start(_: &mut K::Start) -> Self {
/// #         Counting::new()
/// #     }
/// # }
/// # #[call]
/// # async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
/// #     host.with(|mut view| {
/// #         let hlist_pat![total] = view.needs();
/// #         total.0 += 1;
/// #     });
/// # }
/// # /// An item's tally, a component of the part `Counter` only.
/// # #[derive(Numbered)]
/// # struct Tally;
/// # impl<L: PartList<L>> Component<Counter, L, Item> for Tally {
/// #     type Needs = HList![Ref<Total>];
/// #     fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
/// #         Tally
/// #     }
/// # }
/// # #[derive(Numbered)]
/// # struct Counter;
/// # impl<L> Part<L> for Counter {
/// #     type Args = ();
/// #     type Shares = HList![
/// #         Share<Root, HList![Total], HList![Counting]>,
/// #         Share<Item, HList![Count, Tally]>,
/// #     ];
/// # }
/// # struct Q;
/// # composition!(impl for Q: Root, HList![Counter], StillClock, u8);
/// /// Kept by no node of the structure.
/// #[derive(Numbered)]
/// struct Orphan;
/// explain!(Q, Item, needs![Ref<Orphan>]);
/// ```
#[macro_export]
macro_rules! explain {
    ($q:ty, $k:ty, call $c:ty $(,)?) => {
        $crate::__explain_needs!($q, $k, <$c as $crate::call::CallKind<$q, $k>>::Needs);
    };
    ($q:ty, $k:ty, component $c:ident $(:: $cs:ident)* $(<$($g:ty),* $(,)?>)? of $o:ty $(,)?) => {
        $crate::__explain_needs!(
            $q,
            $k,
            $crate::frunk_core::hlist::HCons<
                $crate::__private::This<$c $(:: $cs)* $(<$($g),*>)?>,
                <$c $(:: $cs)* $(<$($g),*>)? as $crate::component::Component<$o, $crate::call::PartsOf<$q>, $k>>::Needs,
            >
        );
        $crate::__explain_build!(
            $q,
            $k,
            <$c $(:: $cs)* $(<$($g),*>)? as $crate::component::Component<$o, $crate::call::PartsOf<$q>, $k>>::BuildNeeds
        );
    };
    ($q:ty, $k:ty, component $c:ty $(,)?) => {
        $crate::__explain_needs!(
            $q,
            $k,
            $crate::frunk_core::hlist::HCons<
                $crate::__private::This<$c>,
                <$c as $crate::component::Component<_, $crate::call::PartsOf<$q>, $k>>::Needs,
            >
        );
        $crate::__explain_build!(
            $q,
            $k,
            <$c as $crate::component::Component<_, $crate::call::PartsOf<$q>, $k>>::BuildNeeds
        );
    };
    ($q:ty, $k:ty, $n:ty $(,)?) => {
        $crate::__explain_needs!($q, $k, $n);
    };
}

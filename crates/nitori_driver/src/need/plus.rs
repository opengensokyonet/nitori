//! `+`: several ways of taking from what is left, each holding alone and
//! used one at a time.
use core::marker::PhantomData;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::{Need, Pos};

/// Several ways of taking from what is left, each of them holding alone,
/// that may overlap: the branches `Bs`, a list of needs, of which the view
/// gives one at a time ([`PlusView::take`]).
///
/// Each branch is checked as a list of its own, so needs inside one branch
/// still must not overlap; the branches are not checked against one
/// another. Against the other needs of the list, a `Plus` takes every
/// component any branch takes: exclusive when a branch takes it exclusive,
/// shared when every branch only reads it. [`needs!`](crate::needs!)
/// writes it as `(A + B)`.
///
/// From a link path, every link core of the connection or the lend
/// point's own, beside the sent records and the planner:
///
/// ```
/// # use nitori_driver::needs;
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(
///     LinkPath,
///     needs![
///         Ref<SentRecords>,
///         (Under<ConnCore, Mut<LinkCore>> + Mut<LinkCore>),
///         Mut<Planner>,
///     ]
/// );
/// ```
///
/// Two needs of one branch that overlap do not compile:
///
/// ```compile_fail,E0277
/// # use nitori_driver::needs;
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(
///     LinkPath,
///     needs![(HList![Mut<LinkCore>, Under<ConnCore, Ref<LinkCore>>] + Ref<Planner>)]
/// );
/// ```
///
/// A need beside the `Plus` that reads a component every branch only
/// reads does not overlap it:
///
/// ```
/// # use nitori_driver::needs;
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(LinkPath, needs![Ref<Planner>, (Ref<Planner> + Ref<LinkCore>)]);
/// ```
///
/// but one beside a branch that takes it exclusive does:
///
/// ```compile_fail,E0277
/// # use nitori_driver::needs;
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(LinkPath, needs![Ref<Planner>, (Mut<Planner> + Ref<LinkCore>)]);
/// ```
///
/// Inside an [`Under`](super::Under), what a branch takes exclusive is at
/// each member or below it, and what it takes above only read: the
/// recoveries of one logical path share its sent records.
///
/// ```
/// # use nitori_driver::needs;
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(
///     Conn,
///     HList![Under<ConnCore, At<Congestion, needs![(Mut<Congestion> + Ref<SentRecords>)]>>]
/// );
/// ```
///
/// ```compile_fail,E0277
/// # use nitori_driver::needs;
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(
///     Conn,
///     HList![Under<ConnCore, At<Congestion, needs![(Mut<Congestion> + Mut<SentRecords>)]>>]
/// );
/// ```
pub struct Plus<Bs>(PhantomData<fn() -> Bs>);
impl<Bs: Branches> Need for Plus<Bs> {
    type View<'t, P: Pos> = PlusView<'t, Bs, P>;
}

/// The branches of a [`Plus`]: a list of needs, and how the view of each
/// is made from what the `Plus` holds.
pub trait Branches: 'static {
    /// For each branch, the function that makes its view from what is held,
    /// `Hd`, borrowed; `Ps` are the branches' positions.
    type Fns<Hd, Ps: Pos>;
}
impl Branches for HNil {
    type Fns<Hd, Ps: Pos> = HNil;
}
impl<B: Need, T: Branches> Branches for HCons<B, T> {
    type Fns<Hd, Ps: Pos> =
        HCons<for<'b> fn(&'b mut Hd) -> B::View<'b, Ps::Head>, T::Fns<Hd, Ps::Tail>>;
}

/// The branch `T` is in the list `Self`, at `I` (inferred).
#[diagnostic::on_unimplemented(
    message = "`{T}` is not a branch of the `Plus` of `{Self}`",
    label = "not one of the branches",
    note = "`take::<T, _>()` takes one of the branches of a `Plus`, written as it is in the need"
)]
pub trait BranchAt<T: Need, I>: Branches {
    /// Its position, among the branches' positions `Ps`.
    type At<Ps: Pos>: Pos;
    /// Its view, made from what is held.
    fn call<'b, Hd, Ps: Pos>(
        fns: &Self::Fns<Hd, Ps>,
        held: &'b mut Hd,
    ) -> T::View<'b, Self::At<Ps>>;
}
impl<T: Need, Tl: Branches> BranchAt<T, Here> for HCons<T, Tl> {
    type At<Ps: Pos> = Ps::Head;
    fn call<'b, Hd, Ps: Pos>(fns: &Self::Fns<Hd, Ps>, held: &'b mut Hd) -> T::View<'b, Ps::Head> {
        (fns.head)(held)
    }
}
impl<H: Need, Tl: BranchAt<T, I>, T: Need, I> BranchAt<T, There<I>> for HCons<H, Tl> {
    type At<Ps: Pos> = Tl::At<Ps::Tail>;
    fn call<'b, Hd, Ps: Pos>(
        fns: &Self::Fns<Hd, Ps>,
        held: &'b mut Hd,
    ) -> T::View<'b, Tl::At<Ps::Tail>> {
        Tl::call::<Hd, Ps::Tail>(&fns.tail, held)
    }
}

/// The view of a [`Plus`] lent for `'t` at the position `P`: what every
/// branch takes, of which one branch at a time is taken
/// ([`take`](Self::take)).
pub struct PlusView<'t, Bs: Branches, P: Pos> {
    held: P::Held<'t>,
    fns: Bs::Fns<P::Held<'t>, P::Inner>,
}
impl<'t, Bs: Branches, P: Pos> PlusView<'t, Bs, P> {
    /// The view of what `held` holds, each branch made by its function.
    pub(crate) fn new(held: P::Held<'t>, fns: Bs::Fns<P::Held<'t>, P::Inner>) -> Self {
        Self { held, fns }
    }
    /// The view of the branch `T`, borrowed from this one; `I` is
    /// inferred: `take::<T, _>()`.
    pub fn take<T: Need, I>(&mut self) -> T::View<'_, <Bs as BranchAt<T, I>>::At<P::Inner>>
    where
        Bs: BranchAt<T, I>,
    {
        Bs::call(&self.fns, &mut self.held)
    }
}

/// A view from which the view of the need `T` is taken, one at a time:
/// that of a branch of a [`Plus`]; `I` is inferred.
pub trait Take<T: Need, I> {
    /// The position `T` is lent at.
    type At: Pos;
    /// The view of `T`, borrowed from this one.
    fn take(&mut self) -> T::View<'_, Self::At>;
}
impl<'t, Bs: BranchAt<T, I>, T: Need, I, P: Pos> Take<T, I> for PlusView<'t, Bs, P> {
    type At = Bs::At<P::Inner>;
    fn take(&mut self) -> T::View<'_, Self::At> {
        Bs::call(&self.fns, &mut self.held)
    }
}

/// A list of needs, with `(A + B)` for a [`Plus`] of the branches `A` and
/// `B`: `needs![A, (B + C), D]` is `HList![A, Plus<HList![B, C]>, D]`.
///
/// A branch is any need, a list among them; a `+` stands only at the top
/// of a parenthesised item.
///
/// ```
/// # use nitori_driver::needs;
/// # use nitori_driver::{__private::doc::*, frunk_core::HList, need::*};
/// lends!(LinkPath, needs![Mut<Planner>, (Ref<LinkCore> + Under<ConnCore, Ref<LinkCore>>)]);
/// ```
#[macro_export]
macro_rules! needs {
    () => { $crate::frunk_core::hlist::HNil };
    (( $($branches:tt)+ ) $(, $($rest:tt)*)?) => {
        $crate::frunk_core::hlist::HCons<
            $crate::__plus!([] [] $($branches)+),
            $crate::needs!($($($rest)*)?),
        >
    };
    ($need:ty $(, $($rest:tt)*)?) => {
        $crate::frunk_core::hlist::HCons<$need, $crate::needs!($($($rest)*)?)>
    };
}

/// The branches of a parenthesised item of [`needs!`], split at `+`.
#[doc(hidden)]
#[macro_export]
macro_rules! __plus {
    ([$([$($done:tt)*])*] [$($cur:tt)*] + $($rest:tt)*) => {
        $crate::__plus!([$([$($done)*])* [$($cur)*]] [] $($rest)*)
    };
    ([$([$($done:tt)*])*] [$($cur:tt)*] $t:tt $($rest:tt)*) => {
        $crate::__plus!([$([$($done)*])*] [$($cur)* $t] $($rest)*)
    };
    ([$([$($done:tt)*])*] [$($cur:tt)*]) => {
        $crate::need::Plus<$crate::frunk_core::HList![$($($done)*,)* $($cur)*]>
    };
}

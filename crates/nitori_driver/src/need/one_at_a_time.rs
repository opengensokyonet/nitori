//! Taking at the members of an [`Under`](super::Under) one at a time.
use alloc::vec::Vec;
use core::marker::PhantomData;

use super::{MemberNeed, Need, Pos, Shape};

/// `T` taken at the members of an [`Under`](super::Under) one member at a
/// time: the members may share what `T` takes exclusive above them, and
/// the view ([`OneView`]) hands out one member's view at a time.
///
/// Every recovery's congestion with the sent records of its logical path,
/// both exclusive, which [`Under`](super::Under) alone refuses since the
/// recoveries of one logical path share the sent records:
///
/// ```
/// # use nitori_driver::{
/// #     frunk_core::HList,
/// #     id::Numbered,
/// #     need::*,
/// #     structure::{AtMostOne, Child, NodeKind, Share, SlotVec},
/// # };
/// # #[derive(Numbered)] struct ConnCore;
/// # #[derive(Numbered)] struct SentRecords;
/// # #[derive(Numbered)] struct LinkCore;
/// # #[derive(Numbered)] struct Planner;
/// # #[derive(Numbered)] struct Congestion;
/// # #[derive(Numbered)] struct Label;
/// # #[derive(Clone, PartialEq, Eq)] struct PathKey(u32);
/// # #[derive(Clone, PartialEq, Eq)] struct LinkId(u32);
/// # struct Conn;
/// # impl NodeKind for Conn { type Children = HList![Child<LogicalPath, SlotVec<PathKey>>]; }
/// # struct LogicalPath;
/// # impl NodeKind for LogicalPath { type Children = HList![Child<LinkPath, SlotVec<LinkId>>]; }
/// # struct LinkPath;
/// # impl NodeKind for LinkPath { type Children = HList![Child<Recovery, AtMostOne>]; }
/// # struct Recovery;
/// # impl NodeKind for Recovery { type Children = HList![]; }
/// # struct V1;
/// # impl Share<Conn> for V1 { type Data = HList![ConnCore, Label]; }
/// # impl Share<LogicalPath> for V1 { type Data = HList![SentRecords]; }
/// # impl Share<LinkPath> for V1 { type Data = HList![LinkCore, Planner, Label]; }
/// # impl Share<Recovery> for V1 { type Data = HList![Congestion]; }
/// # macro_rules! lends {
/// #     ($l:ty, $n:ty) => {
/// #         nitori_driver::__private::assert_lends::<Conn, HList![V1], $l, $n, _, _>()
/// #     };
/// # }
/// lends!(
///     Conn,
///     HList![Under<ConnCore, OneAtATime<At<Congestion, HList![Mut<Congestion>, Mut<SentRecords>]>>>]
/// );
/// ```
pub struct OneAtATime<T>(PhantomData<fn() -> T>);
impl<T: Need> Need for OneAtATime<T> {
    type View<'t, P: Pos> = T::View<'t, P::Inner>;
}
impl<T: Need> MemberNeed for OneAtATime<T> {
    type Collected<'t, P: Pos> = OneView<'t, T, <P::Shape as Shape>::Key, P::Inner>;
}

/// The view of the members of an [`Under`](super::Under) taken
/// [`OneAtATime`], lent for `'t`: their keys `K`, and the view of `T` at
/// the position `P` of one member at a time.
pub struct OneView<'t, T: Need, K, P: Pos> {
    keys: Vec<K>,
    held: P::Held<'t>,
    one: for<'b> fn(&'b mut P::Held<'t>, usize) -> T::View<'b, P::Inner>,
}
impl<'t, T: Need, K, P: Pos> OneView<'t, T, K, P> {
    /// The view of the members under `keys`, from what `held` holds; `one`
    /// makes the view of the member at a place in `keys`.
    pub(crate) fn new(
        keys: Vec<K>,
        held: P::Held<'t>,
        one: for<'b> fn(&'b mut P::Held<'t>, usize) -> T::View<'b, P::Inner>,
    ) -> Self {
        Self { keys, held, one }
    }
    /// The keys of the members.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.keys.iter()
    }
    /// How many members there are.
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    /// Whether there is no member.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    /// The view of the member under `key`, borrowed from this one.
    pub fn get_mut(&mut self, key: &K) -> Option<T::View<'_, P::Inner>>
    where
        K: PartialEq,
    {
        let i = self.keys.iter().position(|k| k == key)?;
        Some((self.one)(&mut self.held, i))
    }
    /// `f` called with every member's key and view, one after another.
    pub fn for_each_mut(&mut self, mut f: impl FnMut(&K, T::View<'_, P::Inner>)) {
        for (i, k) in self.keys.iter().enumerate() {
            f(k, (self.one)(&mut self.held, i));
        }
    }
}

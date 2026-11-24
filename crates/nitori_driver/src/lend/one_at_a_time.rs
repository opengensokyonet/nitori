//! The members of an `Under` taken one at a time: what every member takes
//! is lent once, and each member's view is made from it, lent again, when
//! it is asked for.
use alloc::vec::Vec;
use core::marker::PhantomData;

use super::{
    check::{CheckBody, TakesBody, TakesExclusive},
    list::Len,
    plan::{Plan, Rebor},
    tree::{Collect, Ctx, UnderPos},
    under::{Body, Gather, UnderBody},
};
use crate::need::{Members, OneAtATime, OneView, Pos, Shape};

/// The position of the members taken one at a time: the need at each
/// member at `PT`, its plan `Cs`, the members' full keys `MF`.
pub struct OnePos<PT, Cs, MF>(PhantomData<fn() -> (PT, Cs, MF)>);
impl<PT: Pos, Cs: Plan, MF: 'static> Pos for OnePos<PT, Cs, MF> {
    type Inner = PT;
    type Held<'t> = OneHeld<'t, Cs, MF>;
}

/// What the view of members taken one at a time holds: their full keys
/// and the handles of what they take, lent for `'t`.
pub struct OneHeld<'t, Cs: Plan, MF> {
    members: Vec<MF>,
    hs: Cs::Hs<'t>,
}

/// The view of the member at `i`, at the lend point `MC`, from what is
/// held.
fn one<'t, 'b, MC: Ctx, T: Body<MC, PT>, PT: Pos>(
    held: &'b mut OneHeld<'t, T::Cols, MC::Fk>,
    i: usize,
) -> T::View<'b, PT>
where
    T::Cols: Rebor,
{
    let OneHeld { members, hs } = held;
    T::build(<T::Cols as Rebor>::rebor(hs), &members[i])
}

/// The members, lent one at a time, may take exclusive what they share,
/// but not what the members of an `Under` around them share.
impl<T: UnderBody> UnderBody for OneAtATime<T> {
    type B = T::B;
    type Depth<Rt: Len, Sd> = Sd;
}

impl<MC, AF, LF, IA: 'static, IK: 'static, Sh, T, PT: Pos, Cs, MF>
    Gather<MC, AF, LF, UnderPos<IA, IK, Sh, OnePos<PT, Cs, MF>>> for OneAtATime<T>
where
    MC: Ctx<Fk = MF>,
    MF: Clone + 'static,
    T: UnderBody + Body<MC, PT, Cols = Cs>,
    Cs: Rebor,
    Sh: Collect<AF, MF, LF>,
{
    type Cols = Cs;
    fn gather<'a>(
        rows: &'a [&'a MF],
        hs: Cs::Hs<'a>,
        anchor: &AF,
        at: &LF,
        line: bool,
    ) -> Self::Collected<'a, UnderPos<IA, IK, Sh, OnePos<PT, Cs, MF>>>
    where
        Self: 'a,
    {
        let all = Sh::collect(rows, anchor, at, line, |m| m.clone());
        let (keys, members) = <<Sh::Shape as Shape>::Out<MF> as Members<MF>>::iter(&all)
            .map(|(k, m)| (k.clone(), m.clone()))
            .unzip();
        OneView::new(keys, OneHeld { members, hs }, one::<MC, T, PT>)
    }
}

impl<MC, T: CheckBody<MC, PT>, PT, Cs, MF> CheckBody<MC, OnePos<PT, Cs, MF>> for OneAtATime<T> {}
impl<T: TakesExclusive> TakesExclusive for OneAtATime<T> {
    type Out = T::Out;
}
impl<MC, T: TakesBody<MC, PT>, PT, Cs, MF> TakesBody<MC, OnePos<PT, Cs, MF>> for OneAtATime<T> {
    type Touches = T::Touches;
}

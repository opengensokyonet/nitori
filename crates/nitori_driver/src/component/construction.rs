//! Construction order and its flat component list.
use core::ops::Add;

use frunk_core::hlist::{HCons, HList, HNil};

use super::{ArgsOf, Build, Constructors, Group, One, Part, PartList};
use crate::{
    need::{Need, Pos, View},
    structure::NodeKind,
};

/// The component list and the constructors declared by a share.
pub trait ShareData<O, L, K> {
    type Data: HList + 'static;
    type Constructors: HList + 'static;
}

/// A frunk list of components. Its concatenation is determined by its
/// `HCons` and `HNil` structure.
pub trait ComponentList: HList + 'static {
    type SingleConstructors: HList + 'static;
}
impl ComponentList for HNil {
    type SingleConstructors = HNil;
}
impl<C: 'static, T: ComponentList> ComponentList for HCons<C, T> {
    type SingleConstructors = HCons<One<C>, T::SingleConstructors>;
}

impl<O, L, K> ShareData<O, L, K> for HNil {
    type Data = HNil;
    type Constructors = HNil;
}
impl<O, L, K, C: 'static, T: ComponentList> ShareData<O, L, K> for HCons<C, T> {
    type Data = Self;
    type Constructors = <Self as ComponentList>::SingleConstructors;
}
impl<O, L, K, List: Constructed<O, L, K>> ShareData<O, L, K> for Constructors<List> {
    type Data = List::Data;
    type Constructors = List;
}

/// The flat list of components produced by a constructor list.
pub trait Constructed<O, L, K>: HList + 'static {
    type Data: HList + 'static;
}
impl<O, L, K> Constructed<O, L, K> for HNil {
    type Data = HNil;
}
impl<O, L, K, C, T> Constructed<O, L, K> for HCons<C, T>
where
    O: Part<L>,
    L: PartList<L>,
    K: NodeKind,
    C: Construct<O, L, K> + 'static,
    T: Constructed<O, L, K>,
    C::Output: Add<T::Data, Output: HList + 'static>,
{
    type Data = <C::Output as Add<T::Data>>::Output;
}

/// Run a single constructor and hand its components back as a list.
pub trait Construct<O: Part<L>, L: PartList<L>, K: NodeKind> {
    type BuildNeeds: Need;
    type Output: HList + 'static;

    fn construct<P: Pos>(
        start: &mut K::Start,
        own: &O::Args,
        args: &ArgsOf<L>,
        view: View<'_, Self::BuildNeeds, P>,
    ) -> Self::Output;
}
impl<O: Part<L>, L: PartList<L>, K: NodeKind, C: Build<O, L, K, Output = C>> Construct<O, L, K>
    for One<C>
{
    type BuildNeeds = C::BuildNeeds;
    type Output = HCons<C, HNil>;

    fn construct<P: Pos>(
        start: &mut K::Start,
        own: &O::Args,
        args: &ArgsOf<L>,
        view: View<'_, Self::BuildNeeds, P>,
    ) -> Self::Output {
        HCons {
            head: C::build::<P>(start, own, args, view),
            tail: HNil,
        }
    }
}
impl<O: Part<L>, L: PartList<L>, K: NodeKind, G: Build<O, L, K, Output: ComponentList>>
    Construct<O, L, K> for Group<G>
{
    type BuildNeeds = G::BuildNeeds;
    type Output = G::Output;

    fn construct<P: Pos>(
        start: &mut K::Start,
        own: &O::Args,
        args: &ArgsOf<L>,
        view: View<'_, Self::BuildNeeds, P>,
    ) -> Self::Output {
        G::build::<P>(start, own, args, view)
    }
}

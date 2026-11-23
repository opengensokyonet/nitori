//! The rows of a lending: one table per kind of the structure, one row per
//! node, walked once from the root.
//!
//! A row is a node's full key and each of its components as an
//! `Option<&mut C>`, taken out when a need borrows it exclusive. The
//! nodes' data and children are borrowed apart (`Node { data, children }`),
//! so every component of the tree is borrowed once, without `unsafe`.
use alloc::vec::Vec;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::tree::{FkStep, FlatData, RowFam};
use crate::structure::{Child, ChildStores, KindFor, Node, Store};

/// The rows of the nodes of one kind with full keys `F`, and the tables of
/// the kinds below it.
pub struct Tab<F, Row, Kids> {
    /// One row per node, in the order of the walk.
    pub rows: Vec<(F, Row)>,
    /// One table per child kind, in the order of the children.
    pub kids: Kids,
}
impl<F, Row, Kids: Default> Default for Tab<F, Row, Kids> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            kids: Kids::default(),
        }
    }
}

/// The table of a kind whose nodes have the full keys `F`, for the parts
/// `Pt`.
pub trait KindTab<Pt: 'static, F>: KindFor<Pt> {
    /// The table, borrowing the tree for `'a`.
    type Table<'a>: Default
    where
        Pt: 'a;
    /// Add the row of `node`, kept under the full key `fk`, and the rows of
    /// every node below it.
    fn fill<'a>(node: &'a mut Node<Self, Pt>, fk: F, tab: &mut Self::Table<'a>);
}
impl<K, Pt: 'static, F> KindTab<Pt, F> for K
where
    K: KindFor<Pt, Data = <Pt as crate::structure::Shares<K>>::Data>,
    Pt: crate::structure::Shares<K>,
    <Pt as crate::structure::Shares<K>>::Data: FlatData,
    K::Children: KidTabs<Pt, F>,
{
    type Table<'a>
        = Tab<
        F,
        <<<Pt as crate::structure::Shares<K>>::Data as FlatData>::Flat as RowFam>::Row<'a>,
        <K::Children as KidTabs<Pt, F>>::Tabs<'a>,
    >
    where
        Pt: 'a;
    fn fill<'a>(node: &'a mut Node<K, Pt>, fk: F, tab: &mut Self::Table<'a>) {
        let Node { data, children, .. } = node;
        <K::Children as KidTabs<Pt, F>>::fill(children, &fk, &mut tab.kids);
        tab.rows.push((fk, data.row()));
    }
}

/// The tables of the kinds of a list of children under nodes with the
/// full keys `F`.
pub trait KidTabs<Pt: 'static, F>: ChildStores<Pt> {
    /// One table per child kind.
    type Tabs<'a>: Default + frunk_core::hlist::HList
    where
        Pt: 'a;
    /// Add the rows of every child in `stores`, and of the nodes below.
    fn fill<'a>(stores: &'a mut Self::Stores, fk: &F, tabs: &mut Self::Tabs<'a>);
}
impl<Pt: 'static, F> KidTabs<Pt, F> for HNil {
    type Tabs<'a>
        = HNil
    where
        Pt: 'a;
    fn fill(_: &mut HNil, _: &F, _: &mut HNil) {}
}
impl<Pt: 'static, F, CK, St, T> KidTabs<Pt, F> for HCons<Child<CK, St>, T>
where
    CK: KindTab<Pt, St::Out>,
    St: FkStep<F>,
    T: KidTabs<Pt, F>,
{
    type Tabs<'a>
        = HCons<CK::Table<'a>, T::Tabs<'a>>
    where
        Pt: 'a;
    fn fill<'a>(stores: &'a mut Self::Stores, fk: &F, tabs: &mut Self::Tabs<'a>) {
        let HCons { head, tail } = stores;
        for (key, node) in head.iter_mut() {
            CK::fill(node, St::push(fk, key), &mut tabs.head);
        }
        T::fill(tail, fk, &mut tabs.tail);
    }
}

/// The table of the child kind at the position `J` of a list.
pub trait KidTab<Pt: 'static, F, J>: KidTabs<Pt, F> {
    /// The child kind.
    type Kind: KindTab<Pt, Self::Fk>;
    /// The full key of the child kind's nodes.
    type Fk: Clone + PartialEq + 'static;
    /// Its table.
    fn kid<'a: 'b, 'b>(tabs: &'b Self::Tabs<'a>) -> &'b KidTable<'a, Self, Pt, F, J>;
    /// Its table, exclusive.
    fn kid_mut<'a: 'b, 'b>(tabs: &'b mut Self::Tabs<'a>) -> &'b mut KidTable<'a, Self, Pt, F, J>;
}
/// The table of the child kind at `J` of the list `L`.
pub type KidTable<'a, L, Pt, F, J> =
    <<L as KidTab<Pt, F, J>>::Kind as KindTab<Pt, <L as KidTab<Pt, F, J>>::Fk>>::Table<'a>;
impl<Pt: 'static, F, CK, St, T> KidTab<Pt, F, Here> for HCons<Child<CK, St>, T>
where
    CK: KindTab<Pt, St::Out>,
    St: FkStep<F>,
    T: KidTabs<Pt, F>,
{
    type Kind = CK;
    type Fk = St::Out;
    fn kid<'a: 'b, 'b>(tabs: &'b Self::Tabs<'a>) -> &'b CK::Table<'a> {
        &tabs.head
    }
    fn kid_mut<'a: 'b, 'b>(tabs: &'b mut Self::Tabs<'a>) -> &'b mut CK::Table<'a> {
        &mut tabs.head
    }
}
impl<Pt: 'static, F, CK, St, T, J> KidTab<Pt, F, There<J>> for HCons<Child<CK, St>, T>
where
    CK: KindTab<Pt, St::Out>,
    St: FkStep<F>,
    T: KidTab<Pt, F, J>,
{
    type Kind = T::Kind;
    type Fk = T::Fk;
    fn kid<'a: 'b, 'b>(tabs: &'b Self::Tabs<'a>) -> &'b KidTable<'a, T, Pt, F, J> {
        T::kid(&tabs.tail)
    }
    fn kid_mut<'a: 'b, 'b>(tabs: &'b mut Self::Tabs<'a>) -> &'b mut KidTable<'a, T, Pt, F, J> {
        T::kid_mut(&mut tabs.tail)
    }
}

/// The rows of the kind at the route `Rt` below a kind with full keys `F`.
pub trait TabAt<Pt: 'static, F, Rt>: KindTab<Pt, F> {
    /// The full key of that kind's nodes.
    type AtFk: Clone + PartialEq + 'static;
    /// That kind's components.
    type AtComps: RowFam;
    /// The rows.
    #[allow(clippy::type_complexity)]
    fn rows<'a: 'b, 'b>(
        tab: &'b Self::Table<'a>,
    ) -> &'b Vec<(Self::AtFk, <Self::AtComps as RowFam>::Row<'a>)>;
    /// The rows, exclusive.
    #[allow(clippy::type_complexity)]
    fn rows_mut<'a: 'b, 'b>(
        tab: &'b mut Self::Table<'a>,
    ) -> &'b mut Vec<(Self::AtFk, <Self::AtComps as RowFam>::Row<'a>)>;
}
impl<K, Pt: 'static, F: Clone + PartialEq + 'static> TabAt<Pt, F, HNil> for K
where
    K: KindFor<Pt, Data = <Pt as crate::structure::Shares<K>>::Data>,
    Pt: crate::structure::Shares<K>,
    <Pt as crate::structure::Shares<K>>::Data: FlatData,
    K::Children: KidTabs<Pt, F>,
{
    type AtFk = F;
    type AtComps = <<Pt as crate::structure::Shares<K>>::Data as FlatData>::Flat;
    fn rows<'a: 'b, 'b>(
        tab: &'b Self::Table<'a>,
    ) -> &'b Vec<(F, <Self::AtComps as RowFam>::Row<'a>)> {
        &tab.rows
    }
    fn rows_mut<'a: 'b, 'b>(
        tab: &'b mut Self::Table<'a>,
    ) -> &'b mut Vec<(F, <Self::AtComps as RowFam>::Row<'a>)> {
        &mut tab.rows
    }
}
impl<K, Pt: 'static, F, J, Rt> TabAt<Pt, F, HCons<J, Rt>> for K
where
    K: KindFor<Pt, Data = <Pt as crate::structure::Shares<K>>::Data>,
    Pt: crate::structure::Shares<K>,
    <Pt as crate::structure::Shares<K>>::Data: FlatData,
    K::Children: KidTab<Pt, F, J>,
    <K::Children as KidTab<Pt, F, J>>::Kind: TabAt<Pt, <K::Children as KidTab<Pt, F, J>>::Fk, Rt>,
{
    type AtFk = <<K::Children as KidTab<Pt, F, J>>::Kind as TabAt<
        Pt,
        <K::Children as KidTab<Pt, F, J>>::Fk,
        Rt,
    >>::AtFk;
    type AtComps = <<K::Children as KidTab<Pt, F, J>>::Kind as TabAt<
        Pt,
        <K::Children as KidTab<Pt, F, J>>::Fk,
        Rt,
    >>::AtComps;
    fn rows<'a: 'b, 'b>(
        tab: &'b Self::Table<'a>,
    ) -> &'b Vec<(Self::AtFk, <Self::AtComps as RowFam>::Row<'a>)> {
        <<K::Children as KidTab<Pt, F, J>>::Kind as TabAt<
            Pt,
            <K::Children as KidTab<Pt, F, J>>::Fk,
            Rt,
        >>::rows(<K::Children as KidTab<Pt, F, J>>::kid(&tab.kids))
    }
    fn rows_mut<'a: 'b, 'b>(
        tab: &'b mut Self::Table<'a>,
    ) -> &'b mut Vec<(Self::AtFk, <Self::AtComps as RowFam>::Row<'a>)> {
        <<K::Children as KidTab<Pt, F, J>>::Kind as TabAt<
            Pt,
            <K::Children as KidTab<Pt, F, J>>::Fk,
            Rt,
        >>::rows_mut(<K::Children as KidTab<Pt, F, J>>::kid_mut(&mut tab.kids))
    }
}

/// The component at the position `J` of a row of the components `Self`.
pub trait SlotAt<J>: RowFam {
    /// Its type.
    type C: 'static;
    /// The slot.
    fn slot<'a: 'b, 'b>(row: &'b Self::Row<'a>) -> &'b Option<&'a mut Self::C>;
    /// The slot, exclusive.
    fn slot_mut<'a: 'b, 'b>(row: &'b mut Self::Row<'a>) -> &'b mut Option<&'a mut Self::C>;
}
impl<C: 'static, T: RowFam> SlotAt<Here> for HCons<C, T> {
    type C = C;
    fn slot<'a: 'b, 'b>(row: &'b Self::Row<'a>) -> &'b Option<&'a mut C> {
        &row.head
    }
    fn slot_mut<'a: 'b, 'b>(row: &'b mut Self::Row<'a>) -> &'b mut Option<&'a mut C> {
        &mut row.head
    }
}
impl<H: 'static, T: SlotAt<J>, J> SlotAt<There<J>> for HCons<H, T> {
    type C = T::C;
    fn slot<'a: 'b, 'b>(row: &'b Self::Row<'a>) -> &'b Option<&'a mut T::C> {
        T::slot(&row.tail)
    }
    fn slot_mut<'a: 'b, 'b>(row: &'b mut Self::Row<'a>) -> &'b mut Option<&'a mut T::C> {
        T::slot_mut(&mut row.tail)
    }
}

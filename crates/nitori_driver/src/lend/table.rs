//! The rows of a lending: one table per kind of the structure, one row per
//! node the lending takes from, filled in one walk from the root.
//!
//! What the lending's plan takes is marked first ([`Want`]): the kinds
//! whose rows it takes, and the anchors below which it takes every node of
//! some kind. The walk then goes down the route to the lend point by key,
//! one level at a time, and, below an anchor on that route, through every
//! node that has rows wanted at or below it; nothing else is visited.
//!
//! A row is a node's full key, each of its components as an
//! `Option<&mut C>`, taken out when a need borrows it exclusive, which node
//! it is, and its place in the walk ([`Span`]). The nodes' data and
//! children are borrowed apart (`Node { data, children }`), so every
//! component of the tree is borrowed once, without `unsafe`.
use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::{
    rows::{Rows, Span},
    tree::{FkStep, FlatData, RowFam, SplitKey},
};
use crate::structure::{Child, ChildStores, KindFor, Node, NodeIdent, Store};

/// What a lending's plan wants of one kind's table, marked before the
/// walk.
#[derive(Clone, Copy, Default)]
pub struct Want {
    /// The rows of the kind's nodes are taken.
    pub rows: bool,
    /// The node of this kind on the route to the lend point is an anchor:
    /// what is wanted below it is taken from every node there.
    pub anchor: bool,
    /// Rows are wanted at this kind or below it (worked out from the
    /// others).
    pub reach: bool,
}

/// One row: a node's full key, its components, which node it is, and its
/// place in the walk.
pub struct TabRow<F, Row> {
    /// The full key.
    pub fk: F,
    /// The components.
    pub row: Row,
    /// Which node it is.
    pub ident: NodeIdent,
    /// Its place in the walk.
    pub span: Span,
}

/// The rows of the nodes of one kind with full keys `F`, and the tables of
/// the kinds below it.
pub struct Tab<F, Row, Kids> {
    /// One row per node visited, in the order of the walk.
    pub rows: Rows<TabRow<F, Row>>,
    /// What the plan wants.
    pub want: Want,
    /// Where among the rows the row of this kind's node on the route to the
    /// lend point is, when the walk went through one and rows are wanted.
    pub route: Option<usize>,
    /// The place in the walk of this kind's node on the route, when it is an
    /// anchor and the walk went through it.
    pub anchor: Option<Span>,
    /// One table per child kind, in the order of the children.
    pub kids: Kids,
}
impl<F, Row, Kids: Default> Default for Tab<F, Row, Kids> {
    fn default() -> Self {
        Self {
            rows: Rows::default(),
            want: Want::default(),
            route: None,
            anchor: None,
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
    /// Work out which tables have rows wanted at or below them; whether
    /// this one has.
    fn settle(tab: &mut Self::Table<'_>) -> bool;
    /// Whether rows are wanted at this kind or below it.
    fn reach(tab: &Self::Table<'_>) -> bool;
    /// Add the row of `node`, kept under the full key `fk`, when this
    /// kind's rows are wanted, and those of every node below it whose rows
    /// are wanted; `walk` numbers the nodes visited.
    fn fill<'a>(node: &'a mut Node<Self, Pt>, fk: F, tab: &mut Self::Table<'a>, walk: &mut usize);
    /// Add the rows along the route `Rt` from `node` down to the lend
    /// point, of which `rest` is what is left of the full key, and, below
    /// an anchor (or when `below` already), the rows of every node there;
    /// whether the lend point is there.
    fn fill_route<'a, Rt, Rest>(
        node: &'a mut Node<Self, Pt>,
        fk: F,
        rest: &Rest,
        tab: &mut Self::Table<'a>,
        walk: &mut usize,
        below: bool,
    ) -> bool
    where
        Self::Children: KidRoute<Pt, F, Rt, Rest>;
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
    fn settle(tab: &mut Self::Table<'_>) -> bool {
        let below = <K::Children as KidTabs<Pt, F>>::settle(&mut tab.kids);
        tab.want.reach = tab.want.rows || below;
        tab.want.reach
    }
    fn reach(tab: &Self::Table<'_>) -> bool {
        tab.want.reach
    }
    fn fill<'a>(node: &'a mut Node<K, Pt>, fk: F, tab: &mut Self::Table<'a>, walk: &mut usize) {
        let Node {
            data,
            children,
            ident,
            ..
        } = node;
        let start = *walk;
        *walk += 1;
        <K::Children as KidTabs<Pt, F>>::fill(children, &fk, &mut tab.kids, walk);
        if tab.want.rows {
            // The nodes of one kind are not below one another: pushed after
            // its children, the row is still in the order of the walk.
            tab.rows.push(TabRow {
                fk,
                row: data.row(),
                ident: *ident,
                span: Span { start, end: *walk },
            });
        }
    }
    fn fill_route<'a, Rt, Rest>(
        node: &'a mut Node<K, Pt>,
        fk: F,
        rest: &Rest,
        tab: &mut Self::Table<'a>,
        walk: &mut usize,
        below: bool,
    ) -> bool
    where
        K::Children: KidRoute<Pt, F, Rt, Rest>,
    {
        let Node {
            data,
            children,
            ident,
            ..
        } = node;
        let start = *walk;
        *walk += 1;
        let below = below || tab.want.anchor;
        let found = <K::Children as KidRoute<Pt, F, Rt, Rest>>::fill_route(
            children,
            &fk,
            rest,
            &mut tab.kids,
            walk,
            below,
        );
        let span = Span { start, end: *walk };
        if tab.want.anchor {
            tab.anchor = Some(span);
        }
        if tab.want.rows {
            tab.route = Some(tab.rows.len());
            tab.rows.push(TabRow {
                fk,
                row: data.row(),
                ident: *ident,
                span,
            });
        }
        found
    }
}

/// The tables of the kinds of a list of children under nodes with the
/// full keys `F`.
pub trait KidTabs<Pt: 'static, F>: ChildStores<Pt> {
    /// One table per child kind.
    type Tabs<'a>: Default + frunk_core::hlist::HList
    where
        Pt: 'a;
    /// [`KindTab::settle`] for each child kind; whether any has rows
    /// wanted.
    fn settle(tabs: &mut Self::Tabs<'_>) -> bool;
    /// Add the rows of every child in `stores` of a kind with rows wanted
    /// at or below it, and of the nodes below.
    fn fill<'a>(stores: &'a mut Self::Stores, fk: &F, tabs: &mut Self::Tabs<'a>, walk: &mut usize);
}
impl<Pt: 'static, F> KidTabs<Pt, F> for HNil {
    type Tabs<'a>
        = HNil
    where
        Pt: 'a;
    fn settle(_: &mut HNil) -> bool {
        false
    }
    fn fill(_: &mut HNil, _: &F, _: &mut HNil, _: &mut usize) {}
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
    fn settle(tabs: &mut Self::Tabs<'_>) -> bool {
        let head = CK::settle(&mut tabs.head);
        T::settle(&mut tabs.tail) || head
    }
    fn fill<'a>(stores: &'a mut Self::Stores, fk: &F, tabs: &mut Self::Tabs<'a>, walk: &mut usize) {
        let HCons { head, tail } = stores;
        if CK::reach(&tabs.head) {
            for (key, node) in head.iter_mut() {
                CK::fill(node, St::push(fk, key), &mut tabs.head, walk);
            }
        }
        T::fill(tail, fk, &mut tabs.tail, walk);
    }
}

/// The rows of a list of children along the route `Rt` to the lend point
/// (a position among the children and the route below it, or `HNil` at
/// the lend point), of which `Rest` is what is left of the full key.
pub trait KidRoute<Pt: 'static, F, Rt, Rest>: KidTabs<Pt, F> {
    /// The child on the route by its key, and below an anchor every child
    /// with rows wanted at or below it; whether the lend point is there.
    fn fill_route<'a>(
        stores: &'a mut Self::Stores,
        fk: &F,
        rest: &Rest,
        tabs: &mut Self::Tabs<'a>,
        walk: &mut usize,
        below: bool,
    ) -> bool;
}
/// At the lend point, its children only below an anchor.
impl<Pt: 'static, F, Rest, L: KidTabs<Pt, F>> KidRoute<Pt, F, HNil, Rest> for L {
    fn fill_route<'a>(
        stores: &'a mut L::Stores,
        fk: &F,
        _: &Rest,
        tabs: &mut L::Tabs<'a>,
        walk: &mut usize,
        below: bool,
    ) -> bool {
        if below {
            L::fill(stores, fk, tabs, walk);
        }
        true
    }
}
/// The route goes through the first child kind.
impl<Pt: 'static, F, Rest, CK, St, T, Sub> KidRoute<Pt, F, HCons<Here, Sub>, Rest>
    for HCons<Child<CK, St>, T>
where
    CK: KindTab<Pt, St::Out>,
    St: FkStep<F> + SplitKey<Rest>,
    T: KidTabs<Pt, F>,
    CK::Children: KidRoute<Pt, St::Out, Sub, St::Rest>,
{
    fn fill_route<'a>(
        stores: &'a mut Self::Stores,
        fk: &F,
        rest: &Rest,
        tabs: &mut Self::Tabs<'a>,
        walk: &mut usize,
        below: bool,
    ) -> bool {
        let HCons { head, tail } = stores;
        let (key, rest) = St::split_key(rest);
        let found = if below {
            let reach = CK::reach(&tabs.head);
            let mut found = false;
            for (k, node) in head.iter_mut() {
                if *k == key {
                    found = CK::fill_route::<Sub, St::Rest>(
                        node,
                        St::push(fk, k),
                        rest,
                        &mut tabs.head,
                        walk,
                        true,
                    );
                } else if reach {
                    CK::fill(node, St::push(fk, k), &mut tabs.head, walk);
                }
            }
            found
        } else {
            match head.find(&key).and_then(|h| head.get_mut(h)) {
                Some(node) => CK::fill_route::<Sub, St::Rest>(
                    node,
                    St::push(fk, &key),
                    rest,
                    &mut tabs.head,
                    walk,
                    false,
                ),
                None => false,
            }
        };
        if below {
            T::fill(tail, fk, &mut tabs.tail, walk);
        }
        found
    }
}
/// The route goes through a later child kind.
impl<Pt: 'static, F, Rest, CK, St, T, J, Sub> KidRoute<Pt, F, HCons<There<J>, Sub>, Rest>
    for HCons<Child<CK, St>, T>
where
    CK: KindTab<Pt, St::Out>,
    St: FkStep<F>,
    T: KidRoute<Pt, F, HCons<J, Sub>, Rest>,
{
    fn fill_route<'a>(
        stores: &'a mut Self::Stores,
        fk: &F,
        rest: &Rest,
        tabs: &mut Self::Tabs<'a>,
        walk: &mut usize,
        below: bool,
    ) -> bool {
        let HCons { head, tail } = stores;
        if below && CK::reach(&tabs.head) {
            for (key, node) in head.iter_mut() {
                CK::fill(node, St::push(fk, key), &mut tabs.head, walk);
            }
        }
        T::fill_route(tail, fk, rest, &mut tabs.tail, walk, below)
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
    ) -> &'b [TabRow<Self::AtFk, <Self::AtComps as RowFam>::Row<'a>>];
    /// The rows, exclusive.
    #[allow(clippy::type_complexity)]
    fn rows_mut<'a: 'b, 'b>(
        tab: &'b mut Self::Table<'a>,
    ) -> &'b mut [TabRow<Self::AtFk, <Self::AtComps as RowFam>::Row<'a>>];
    /// What the plan wants of the table, to mark it.
    fn want_mut<'a: 'b, 'b>(tab: &'b mut Self::Table<'a>) -> &'b mut Want;
    /// Where among the rows the row of that kind's node on the route is
    /// ([`Tab::route`]).
    fn route(tab: &Self::Table<'_>) -> Option<usize>;
    /// The place in the walk of that kind's node on the route, an anchor
    /// ([`Tab::anchor`]).
    fn anchor(tab: &Self::Table<'_>) -> Option<Span>;
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
    ) -> &'b [TabRow<F, <Self::AtComps as RowFam>::Row<'a>>] {
        &tab.rows
    }
    fn rows_mut<'a: 'b, 'b>(
        tab: &'b mut Self::Table<'a>,
    ) -> &'b mut [TabRow<F, <Self::AtComps as RowFam>::Row<'a>>] {
        &mut tab.rows
    }
    fn want_mut<'a: 'b, 'b>(tab: &'b mut Self::Table<'a>) -> &'b mut Want {
        &mut tab.want
    }
    fn route(tab: &Self::Table<'_>) -> Option<usize> {
        tab.route
    }
    fn anchor(tab: &Self::Table<'_>) -> Option<Span> {
        tab.anchor
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
    ) -> &'b [TabRow<Self::AtFk, <Self::AtComps as RowFam>::Row<'a>>] {
        <<K::Children as KidTab<Pt, F, J>>::Kind as TabAt<
            Pt,
            <K::Children as KidTab<Pt, F, J>>::Fk,
            Rt,
        >>::rows(<K::Children as KidTab<Pt, F, J>>::kid(&tab.kids))
    }
    fn rows_mut<'a: 'b, 'b>(
        tab: &'b mut Self::Table<'a>,
    ) -> &'b mut [TabRow<Self::AtFk, <Self::AtComps as RowFam>::Row<'a>>] {
        <<K::Children as KidTab<Pt, F, J>>::Kind as TabAt<
            Pt,
            <K::Children as KidTab<Pt, F, J>>::Fk,
            Rt,
        >>::rows_mut(<K::Children as KidTab<Pt, F, J>>::kid_mut(&mut tab.kids))
    }
    fn want_mut<'a: 'b, 'b>(tab: &'b mut Self::Table<'a>) -> &'b mut Want {
        <<K::Children as KidTab<Pt, F, J>>::Kind as TabAt<
            Pt,
            <K::Children as KidTab<Pt, F, J>>::Fk,
            Rt,
        >>::want_mut(<K::Children as KidTab<Pt, F, J>>::kid_mut(&mut tab.kids))
    }
    fn route(tab: &Self::Table<'_>) -> Option<usize> {
        <<K::Children as KidTab<Pt, F, J>>::Kind as TabAt<
            Pt,
            <K::Children as KidTab<Pt, F, J>>::Fk,
            Rt,
        >>::route(<K::Children as KidTab<Pt, F, J>>::kid(&tab.kids))
    }
    fn anchor(tab: &Self::Table<'_>) -> Option<Span> {
        <<K::Children as KidTab<Pt, F, J>>::Kind as TabAt<
            Pt,
            <K::Children as KidTab<Pt, F, J>>::Fk,
            Rt,
        >>::anchor(<K::Children as KidTab<Pt, F, J>>::kid(&tab.kids))
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

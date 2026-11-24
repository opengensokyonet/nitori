//! The pack of a composition: for every kind of its structure, what the
//! composition proved of it, reached through what is declared here; and
//! the entries the driver lends through it.
//!
//! The pack is a list with one item for each kind of the structure, in the
//! order of [`KindsOf`]: the positions of every call ([`CallList`]) and
//! every component ([`CompList`]) of the kind, of its build, and of the
//! reaction points the driver fires at it ([`KindOps`]). The composition
//! defines it as an opaque type, so code generic over the composition uses
//! only these traits; an empty list ends in a filler ([`NoKind`]) that is
//! never reached.
use core::{
    marker::PhantomData,
    pin::Pin,
    task::{self, Poll},
};

use frunk_core::{
    hlist::{HCons, HNil, Selector},
    indices::{Here, There},
};

use super::{
    Composition, ContextOf, KindsOf, PartsOf, PriorityOf, RootNode, RootOf,
    host::CallHost,
    kinds::{FindKind, ListRest, PartsCalls, PartsComps, RequestLists},
};
use crate::{
    component::{ArgsOf, EndCause},
    driver::{ChildOpsList, Core, KindTable, ListsAt, TablesAt, TailLists, TailTables},
    id::Numbered,
    lend::{
        PointCx, PointFk,
        build::{NewPointCx, NewPointFk},
        tree::{Ctx, InKid, IsSelf, Locate},
    },
    structure::{KindFor, MoveError, NoChild, NodeIdent, NodeKind, resolve_ident},
};

// ------------------------------------------------------------------ kinds

/// A kind of the structure of `Q` as the pack reads it.
pub trait KindDesc<Q: Composition> {
    /// The kind ([`NoChild`] for the filler).
    type Kind: NodeKind;
    /// The way down to it from the root.
    type Il;
    /// The full key of its nodes.
    type Fk: Clone + PartialEq + 'static;
    /// What a new node of it is built at: its full key, or `()` for the
    /// root.
    type NewAt;
    /// The parts' shares in a node of it.
    type Data;
    /// The kinds of its calls, in the order they start.
    type Calls: ListRest;
    /// Its components, each a [`Comp`](super::Comp), in the order of the
    /// parts.
    type Comps: ListRest;
    /// What a new node with the full key `fk` is built at.
    fn new_at(fk: &Self::Fk) -> Self::NewAt;
}
/// The kind `K` as an item of the pack's list.
pub struct Desc<K>(PhantomData<fn() -> K>);
/// The filler after the last kind of the pack's list.
pub enum NoKind {}

/// What a new node of the kind `K` is built at, `Self` being the way
/// down to it: `()` for the root, its full key otherwise.
pub trait NewAt<Q: Composition, K> {
    /// What it is built at.
    type Out;
}
impl<Q: Composition, K> NewAt<Q, K> for IsSelf {
    type Out = ();
}
/// What a new node of the kind `K` with the full key `F` is built at,
/// `Self` being the way down to it.
pub trait NewAtOf<Q: Composition, K, F>: NewAt<Q, K> {
    /// What it is built at.
    fn new_at(fk: &F) -> Self::Out;
}
impl<Q: Composition, K, F> NewAtOf<Q, K, F> for IsSelf {
    fn new_at(_: &F) {}
}
impl<Q: Composition, K, F: Clone, I> NewAtOf<Q, K, F> for InKid<I>
where
    InKid<I>: NewAt<Q, K, Out = F>,
{
    fn new_at(fk: &F) -> F {
        fk.clone()
    }
}
impl<Q: Composition, K, I> NewAt<Q, K> for InKid<I>
where
    RootOf<Q>: Locate<K, InKid<I>>,
    NewPointCx<RootOf<Q>, PartsOf<Q>, K, InKid<I>, ContextOf<Q>>: Ctx,
{
    type Out = NewPointFk<RootOf<Q>, PartsOf<Q>, K, InKid<I>, ContextOf<Q>>;
}

type FoundIl<Q, K> = <RootOf<Q> as FindKind<K>>::Il;
impl<Q: Composition, K> KindDesc<Q> for Desc<K>
where
    K: KindFor<PartsOf<Q>> + Numbered,
    RootOf<Q>: FindKind<K> + Locate<K, FoundIl<Q, K>>,
    PointCx<RootOf<Q>, PartsOf<Q>, K, FoundIl<Q, K>, ContextOf<Q>>: Ctx,
    FoundIl<Q, K>: NewAtOf<Q, K, PointFk<RootOf<Q>, PartsOf<Q>, K, FoundIl<Q, K>, ContextOf<Q>>>,
    PartsOf<Q>: PartsCalls<K> + PartsComps<K>,
{
    type Kind = K;
    type Il = FoundIl<Q, K>;
    type Fk = PointFk<RootOf<Q>, PartsOf<Q>, K, FoundIl<Q, K>, ContextOf<Q>>;
    type NewAt = <FoundIl<Q, K> as NewAt<Q, K>>::Out;
    type Data = <K as KindFor<PartsOf<Q>>>::Data;
    type Calls = <PartsOf<Q> as PartsCalls<K>>::Calls;
    type Comps = <PartsOf<Q> as PartsComps<K>>::Comps;
    fn new_at(fk: &Self::Fk) -> Self::NewAt {
        <FoundIl<Q, K> as NewAtOf<Q, K, Self::Fk>>::new_at(fk)
    }
}
impl<Q: Composition> KindDesc<Q> for NoKind {
    type Kind = NoChild;
    type Il = ();
    type Fk = !;
    type NewAt = !;
    type Data = ();
    type Calls = HNil;
    type Comps = HNil;
    fn new_at(fk: &!) -> ! {
        match *fk {}
    }
}

/// The way down to the kind `K` of `Q`'s structure.
pub(crate) type IlOf<Q, K> = <Desc<K> as KindDesc<Q>>::Il;
/// The full key of the nodes of the kind `K` of `Q`'s tree.
pub(crate) type FkOf<Q, K> = <Desc<K> as KindDesc<Q>>::Fk;
/// What the kind `D` of the pack starts from.
pub(crate) type StartOf<Q, D> = <<D as KindDesc<Q>>::Kind as NodeKind>::Start;
/// Why the kind `D` of the pack ends.
pub(crate) type EndReasonOf<Q, D> = <<D as KindDesc<Q>>::Kind as NodeKind>::EndReason;

/// A list of kinds of `Q`'s structure as head and tail (the empty list:
/// [`NoKind`] and itself), with a request box for each.
pub trait KindList<Q: Composition>: RequestLists<RootOf<Q>> {
    /// The first kind.
    type Head: KindDesc<Q>;
    /// The others.
    type Tail: KindList<Q>;
}
impl<Q: Composition> KindList<Q> for HNil {
    type Head = NoKind;
    type Tail = HNil;
}
impl<Q: Composition, K: NodeKind + Numbered, T: KindList<Q>> KindList<Q> for HCons<K, T>
where
    Desc<K>: KindDesc<Q>,
{
    type Head = Desc<K>;
    type Tail = T;
}

// ------------------------------------------------------------------ the pack

/// What the driver keeps of the nodes of the kind `D`, proved as `O`.
pub type TableOf<Q, D, O> = KindTable<
    <D as KindDesc<Q>>::Fk,
    <<O as KindOps<Q, D>>::Calls as CallList<Q, D, <D as KindDesc<Q>>::Calls>>::Slots,
>;

/// Code that runs on one kind of the structure, whichever it is: what the
/// driver does at a node it keeps by number.
pub trait KindVisit<Q: Composition> {
    /// What it gives back.
    type Out;
    /// Run on the kind `D`, proved as `O`, whose nodes the driver keeps
    /// in `table`.
    fn visit<D: KindDesc<Q>, O: KindOps<Q, D>>(self, table: &mut TableOf<Q, D, O>) -> Self::Out;
}

/// The pack of the kinds `L` of `Q`: one item per kind.
pub trait PackList<Q: Composition, L: KindList<Q>> {
    /// What was proved of the first kind.
    type Head: KindOps<Q, L::Head>;
    /// The others.
    type Tail: PackList<Q, L::Tail>;
    /// What the driver keeps of the nodes of every kind from the first on.
    type Tables: 'static;
    /// Every table, empty.
    fn tables() -> Self::Tables;
    /// The first kind's table.
    fn head_table(tables: &mut Self::Tables) -> &mut TableOf<Q, L::Head, Self::Head>;
    /// The others' tables.
    fn tail_tables(tables: &mut Self::Tables) -> &mut <Self::Tail as PackList<Q, L::Tail>>::Tables;
    /// Run `v` on the kind `kind`, counted from the first; `None` when
    /// there are fewer kinds.
    fn visit<V: KindVisit<Q>>(tables: &mut Self::Tables, kind: usize, v: V) -> Option<V::Out>;
    /// Take the first request queued in the box of the kind `kind`,
    /// counted from the first, and hand it to that kind's handler when
    /// its node is there; `A` reaches these tables among the driver's,
    /// `LA` these boxes among the context's.
    #[allow(private_interfaces)]
    fn handle_first<
        A: TablesAt<Q, Self::Tables>,
        LA: ListsAt<Q, <L as RequestLists<RootOf<Q>>>::Lists>,
    >(
        core: &mut Core<Q>,
        kind: usize,
    );
}
impl<Q: Composition> PackList<Q, HNil> for HNil {
    type Head = NoKindPos;
    type Tail = HNil;
    type Tables = TableOf<Q, NoKind, NoKindPos>;
    fn tables() -> Self::Tables {
        KindTable::default()
    }
    fn head_table(tables: &mut Self::Tables) -> &mut Self::Tables {
        tables
    }
    fn tail_tables(tables: &mut Self::Tables) -> &mut Self::Tables {
        tables
    }
    fn visit<V: KindVisit<Q>>(_: &mut Self::Tables, _: usize, _: V) -> Option<V::Out> {
        None
    }
    #[allow(private_interfaces)]
    fn handle_first<A: TablesAt<Q, Self::Tables>, LA: ListsAt<Q, HNil>>(_: &mut Core<Q>, _: usize) {
    }
}
impl<Q: Composition, K: NodeKind + Numbered, T: KindList<Q>, P, PT> PackList<Q, HCons<K, T>>
    for HCons<P, PT>
where
    Desc<K>: KindDesc<Q, Kind = K>,
    P: KindOps<Q, Desc<K>>,
    PT: PackList<Q, T>,
{
    type Head = P;
    type Tail = PT;
    type Tables = HCons<TableOf<Q, Desc<K>, P>, PT::Tables>;
    fn tables() -> Self::Tables {
        HCons {
            head: KindTable::default(),
            tail: PT::tables(),
        }
    }
    fn head_table(tables: &mut Self::Tables) -> &mut TableOf<Q, Desc<K>, P> {
        &mut tables.head
    }
    fn tail_tables(tables: &mut Self::Tables) -> &mut PT::Tables {
        &mut tables.tail
    }
    fn visit<V: KindVisit<Q>>(tables: &mut Self::Tables, kind: usize, v: V) -> Option<V::Out> {
        match kind.checked_sub(1) {
            None => Some(v.visit::<Desc<K>, P>(&mut tables.head)),
            Some(rest) => PT::visit(&mut tables.tail, rest, v),
        }
    }
    #[allow(private_interfaces)]
    fn handle_first<
        A: TablesAt<Q, Self::Tables>,
        LA: ListsAt<Q, <HCons<K, T> as RequestLists<RootOf<Q>>>::Lists>,
    >(
        core: &mut Core<Q>,
        kind: usize,
    ) {
        type Lists<Q, K, T> = <HCons<K, T> as RequestLists<RootOf<Q>>>::Lists;
        if let Some(rest) = kind.checked_sub(1) {
            PT::handle_first::<TailTables<A, Self::Tables>, TailLists<LA, Lists<Q, K, T>>>(
                core, rest,
            );
            return;
        }
        let Some((_, request)) = LA::get(&mut core.cx).head.pop() else {
            return;
        };
        // A request for a node not there is dropped.
        let Some(ident) = resolve_ident(&core.root, request.at.route()) else {
            return;
        };
        if core
            .nodes
            .live(ident.number)
            .is_none_or(|n| n.serial != ident.serial)
        {
            return;
        }
        let Some(fk) = A::get(&mut core.tables)
            .head
            .nodes
            .get(&ident.number)
            .map(|e| e.fk.clone())
        else {
            return;
        };
        P::handle(core, ident.number, fk, request.request);
    }
}

/// What the composition proved of the kind `D` of `Q`: its calls, its
/// components, its build and the reaction points the driver fires at it.
pub trait KindOps<Q: Composition, D: KindDesc<Q>> {
    /// Its calls.
    type Calls: CallList<Q, D, D::Calls>;
    /// Its components.
    type Comps: CompList<Q, D, D::Comps>;
    /// The shares of a new node at `at`, every component built in the
    /// order of the parts from `start`, the arguments and its build needs;
    /// `None` when its parent is not in the tree.
    fn build(
        root: &mut RootNode<Q>,
        at: &D::NewAt,
        start: &mut StartOf<Q, D>,
        args: &ArgsOf<PartsOf<Q>>,
        cx: &mut ContextOf<Q>,
    ) -> Option<D::Data>;
    /// Fire [`NodeStarted`](crate::react::NodeStarted) at the node at
    /// `at`; `None` when it is not in the tree.
    fn started(root: &mut RootNode<Q>, at: &D::Fk, cx: &mut ContextOf<Q>) -> Option<()>;
    /// Fire [`NodeEnding`](crate::react::NodeEnding) at the node at
    /// `at`; `None` when it is not in the tree.
    fn ending(root: &mut RootNode<Q>, at: &D::Fk, cx: &mut ContextOf<Q>) -> Option<()>;
    /// [`build`](Self::build), for the new node with the full key `at`.
    fn build_at(
        root: &mut RootNode<Q>,
        at: &D::Fk,
        start: &mut StartOf<Q, D>,
        args: &ArgsOf<PartsOf<Q>>,
        cx: &mut ContextOf<Q>,
    ) -> Option<D::Data>;
    /// Whether no node is kept at `at`: `None` when its parent is not
    /// there, or it is the root.
    fn vacant(root: &RootNode<Q>, at: &D::Fk) -> Option<bool>;
    /// Put a new node with the shares `data` into the tree at `at`, kept
    /// by the driver under `number`: which node it is and which its parent
    /// is. The shares come back when its parent is not there or keeps
    /// another node under its key, or it is the root.
    fn insert(
        root: &mut RootNode<Q>,
        at: &D::Fk,
        data: D::Data,
        number: u32,
    ) -> Result<(NodeIdent, NodeIdent), D::Data>;
    /// Take the node at `at` out of the tree, with every node below it;
    /// `false` when it is not there, or it is the root.
    fn remove(root: &mut RootNode<Q>, at: &D::Fk) -> bool;
    /// What an edit handle does on each child of the kind.
    type Children: ChildOpsList<Q, D::Fk, <D::Kind as NodeKind>::Children>;
    /// Hand `request` to the kind's handler, for the node `number` with
    /// the full key `fk`.
    #[allow(private_interfaces)]
    fn handle(core: &mut Core<Q>, number: u32, fk: D::Fk, request: <D::Kind as NodeKind>::Request);
    /// Which node is at `at`, when one is.
    fn ident(root: &RootNode<Q>, at: &D::Fk) -> Option<NodeIdent>;
    /// Move the node at `from`, with every node below it, to `to`, under
    /// another parent, keeping its key: which node its new parent is.
    /// Nothing moves when it is not there, the new parent is not there or
    /// keeps a node under the key, or it is the root.
    fn transplant(root: &mut RootNode<Q>, from: &D::Fk, to: &D::Fk)
    -> Result<NodeIdent, MoveError>;
}
/// The filler's item of the pack.
pub struct NoKindPos;
impl<Q: Composition> KindOps<Q, NoKind> for NoKindPos {
    type Calls = HNil;
    type Comps = HNil;
    fn build(
        _: &mut RootNode<Q>,
        at: &!,
        _: &mut !,
        _: &ArgsOf<PartsOf<Q>>,
        _: &mut ContextOf<Q>,
    ) -> Option<()> {
        match *at {}
    }
    fn started(_: &mut RootNode<Q>, at: &!, _: &mut ContextOf<Q>) -> Option<()> {
        match *at {}
    }
    fn ending(_: &mut RootNode<Q>, at: &!, _: &mut ContextOf<Q>) -> Option<()> {
        match *at {}
    }
    fn build_at(
        _: &mut RootNode<Q>,
        at: &!,
        _: &mut !,
        _: &ArgsOf<PartsOf<Q>>,
        _: &mut ContextOf<Q>,
    ) -> Option<()> {
        match *at {}
    }
    fn vacant(_: &RootNode<Q>, at: &!) -> Option<bool> {
        match *at {}
    }
    fn insert(_: &mut RootNode<Q>, at: &!, _: (), _: u32) -> Result<(NodeIdent, NodeIdent), ()> {
        match *at {}
    }
    fn remove(_: &mut RootNode<Q>, at: &!) -> bool {
        match *at {}
    }
    type Children = HNil;
    #[allow(private_interfaces)]
    fn handle(_: &mut Core<Q>, _: u32, fk: !, _: !) {
        fk
    }
    fn ident(_: &RootNode<Q>, at: &!) -> Option<NodeIdent> {
        match *at {}
    }
    fn transplant(_: &mut RootNode<Q>, at: &!, _: &!) -> Result<NodeIdent, MoveError> {
        match *at {}
    }
}

/// What the composition proved of the calls `Cs` of the kind `D`: the
/// first one's, and the others'.
pub trait CallList<Q: Composition, D: KindDesc<Q>, Cs: ListRest> {
    /// The first call as it runs between polls (`()` for none).
    type HeadRunning;
    /// The others.
    type Tail: CallList<Q, D, Cs::Rest>;
    /// The first call of a new node, from what is left of its start.
    fn start_head(start: &mut StartOf<Q, D>) -> Self::HeadRunning;
    /// The calls from the first on as the driver keeps them: each in its
    /// box, or none once it returned.
    type Slots: 'static;
    /// How many calls there are from the first on.
    const COUNT: u32;
    /// Lend the first call its needs at the node at `at`, with what the
    /// driver keeps of it, `host`, and poll it once with `wcx`: `Ready`
    /// when it returned. `None` when the node is not in the tree.
    fn poll_head(
        root: &mut RootNode<Q>,
        at: &D::Fk,
        cx: &mut ContextOf<Q>,
        call: Pin<&mut Self::HeadRunning>,
        host: &mut CallHost<PriorityOf<Q>>,
        wcx: &mut task::Context<'_>,
    ) -> Option<Poll<()>>;
    /// Every call of a new node, from what is left of its start, in order,
    /// each in its box.
    fn start_slots(start: &mut StartOf<Q, D>) -> Self::Slots;
    /// Poll the call `j` of `slots`, from the first on, as
    /// [`poll_head`](Self::poll_head) does; a call that returned is
    /// dropped.
    #[allow(clippy::too_many_arguments)]
    fn poll_slot(
        root: &mut RootNode<Q>,
        at: &D::Fk,
        cx: &mut ContextOf<Q>,
        slots: &mut Self::Slots,
        j: u32,
        host: &mut CallHost<PriorityOf<Q>>,
        wcx: &mut task::Context<'_>,
    ) -> SlotPoll;
}
/// What polling a call the driver keeps gave.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotPoll {
    /// The node is not in the tree.
    NotThere,
    /// No call there: it returned before.
    NoCall,
    /// It returned now, and was dropped.
    Returned,
    /// It waits.
    Pending,
}
impl<Q: Composition, D: KindDesc<Q>> CallList<Q, D, HNil> for HNil {
    type HeadRunning = ();
    type Tail = HNil;
    type Slots = ();
    const COUNT: u32 = 0;
    fn start_head(_: &mut StartOf<Q, D>) {}
    fn poll_head(
        _: &mut RootNode<Q>,
        _: &D::Fk,
        _: &mut ContextOf<Q>,
        _: Pin<&mut ()>,
        _: &mut CallHost<PriorityOf<Q>>,
        _: &mut task::Context<'_>,
    ) -> Option<Poll<()>> {
        Some(Poll::Ready(()))
    }
    fn start_slots(_: &mut StartOf<Q, D>) {}
    fn poll_slot(
        _: &mut RootNode<Q>,
        _: &D::Fk,
        _: &mut ContextOf<Q>,
        _: &mut (),
        _: u32,
        _: &mut CallHost<PriorityOf<Q>>,
        _: &mut task::Context<'_>,
    ) -> SlotPoll {
        SlotPoll::NoCall
    }
}

/// What the composition proved of the components `Cs` of the kind `D`:
/// the first one's, and the others'.
pub trait CompList<Q: Composition, D: KindDesc<Q>, Cs: ListRest> {
    /// The others.
    type Tail: CompList<Q, D, Cs::Rest>;
    /// Wake the first component at the node at `at`; `None` when it is
    /// not in the tree.
    fn wake_head(root: &mut RootNode<Q>, at: &D::Fk, cx: &mut ContextOf<Q>) -> Option<()>;
    /// End the first component at the node at `at` for `cause`; `None`
    /// when it is not in the tree.
    fn end_head(
        root: &mut RootNode<Q>,
        at: &D::Fk,
        cx: &mut ContextOf<Q>,
        cause: EndCause<'_, EndReasonOf<Q, D>>,
    ) -> Option<()>;
    /// Wake the component numbered `number`, from the first on, at the
    /// node at `at`; `None` when none has that number, `Some(None)` when
    /// the node is not in the tree.
    fn wake_number(
        root: &mut RootNode<Q>,
        at: &D::Fk,
        cx: &mut ContextOf<Q>,
        number: u64,
    ) -> Option<Option<()>>;
    /// End every component from the first on, in order, at the node at
    /// `at`, for `cause`.
    fn end_all(
        root: &mut RootNode<Q>,
        at: &D::Fk,
        cx: &mut ContextOf<Q>,
        cause: EndCause<'_, EndReasonOf<Q, D>>,
    );
}
impl<Q: Composition, D: KindDesc<Q>> CompList<Q, D, HNil> for HNil {
    type Tail = HNil;
    fn wake_number(
        _: &mut RootNode<Q>,
        _: &D::Fk,
        _: &mut ContextOf<Q>,
        _: u64,
    ) -> Option<Option<()>> {
        None
    }
    fn end_all(
        _: &mut RootNode<Q>,
        _: &D::Fk,
        _: &mut ContextOf<Q>,
        _: EndCause<'_, EndReasonOf<Q, D>>,
    ) {
    }
    fn wake_head(_: &mut RootNode<Q>, _: &D::Fk, _: &mut ContextOf<Q>) -> Option<()> {
        Some(())
    }
    fn end_head(
        _: &mut RootNode<Q>,
        _: &D::Fk,
        _: &mut ContextOf<Q>,
        _: EndCause<'_, EndReasonOf<Q, D>>,
    ) -> Option<()> {
        Some(())
    }
}

// ------------------------------------------------------------------ walking the pack

/// The item of the pack `Self` at the position `I` of the kinds `L`.
pub(crate) trait PackNth<Q: Composition, L: KindList<Q>, I> {
    /// The kinds from that position on.
    type List: KindList<Q>;
    /// The item.
    type Out: KindOps<Q, <Self::List as KindList<Q>>::Head>;
}
impl<Q: Composition, L: KindList<Q>, P: PackList<Q, L>> PackNth<Q, L, Here> for P {
    type List = L;
    type Out = P::Head;
}
impl<Q: Composition, L: KindList<Q>, P: PackList<Q, L>, I> PackNth<Q, L, There<I>> for P
where
    P::Tail: PackNth<Q, L::Tail, I>,
{
    type List = <P::Tail as PackNth<Q, L::Tail, I>>::List;
    type Out = <P::Tail as PackNth<Q, L::Tail, I>>::Out;
}

/// The calls `Self` from the position `J` of the calls `Cs` on.
pub(crate) trait NthCall<Q: Composition, D: KindDesc<Q>, Cs: ListRest, J> {
    /// The kinds of the calls from that position on.
    type Cs: ListRest;
    /// What was proved of them.
    type Pos: CallList<Q, D, Self::Cs>;
}
impl<Q: Composition, D: KindDesc<Q>, Cs: ListRest, P: CallList<Q, D, Cs>> NthCall<Q, D, Cs, Here>
    for P
{
    type Cs = Cs;
    type Pos = P;
}
impl<Q: Composition, D: KindDesc<Q>, Cs: ListRest, P: CallList<Q, D, Cs>, J>
    NthCall<Q, D, Cs, There<J>> for P
where
    P::Tail: NthCall<Q, D, Cs::Rest, J>,
{
    type Cs = <P::Tail as NthCall<Q, D, Cs::Rest, J>>::Cs;
    type Pos = <P::Tail as NthCall<Q, D, Cs::Rest, J>>::Pos;
}

/// The components `Self` from the position `J` of the components `Cs`
/// on.
pub(crate) trait NthComp<Q: Composition, D: KindDesc<Q>, Cs: ListRest, J> {
    /// The components from that position on.
    type Cs: ListRest;
    /// What was proved of them.
    type Pos: CompList<Q, D, Self::Cs>;
}
impl<Q: Composition, D: KindDesc<Q>, Cs: ListRest, P: CompList<Q, D, Cs>> NthComp<Q, D, Cs, Here>
    for P
{
    type Cs = Cs;
    type Pos = P;
}
impl<Q: Composition, D: KindDesc<Q>, Cs: ListRest, P: CompList<Q, D, Cs>, J>
    NthComp<Q, D, Cs, There<J>> for P
where
    P::Tail: NthComp<Q, D, Cs::Rest, J>,
{
    type Cs = <P::Tail as NthComp<Q, D, Cs::Rest, J>>::Cs;
    type Pos = <P::Tail as NthComp<Q, D, Cs::Rest, J>>::Pos;
}

/// Every call `Cs` of a new node of the kind `D` started, in order.
pub(crate) trait StartCalls<Q: Composition, D: KindDesc<Q>, Cs: ListRest>:
    CallList<Q, D, Cs>
{
    /// The calls as they run, an `HList`.
    type Running;
    /// Start them all from what is left of the start, in order.
    fn start_all(start: &mut StartOf<Q, D>) -> Self::Running;
}
impl<Q: Composition, D: KindDesc<Q>, P: CallList<Q, D, HNil>> StartCalls<Q, D, HNil> for P {
    type Running = HNil;
    fn start_all(_: &mut StartOf<Q, D>) -> HNil {
        HNil
    }
}
impl<Q: Composition, D: KindDesc<Q>, C, Ct: ListRest, P> StartCalls<Q, D, HCons<C, Ct>> for P
where
    P: CallList<Q, D, HCons<C, Ct>>,
    P::Tail: StartCalls<Q, D, Ct>,
{
    type Running = HCons<P::HeadRunning, <P::Tail as StartCalls<Q, D, Ct>>::Running>;
    fn start_all(start: &mut StartOf<Q, D>) -> Self::Running {
        let head = P::start_head(start);
        HCons {
            head,
            tail: <P::Tail as StartCalls<Q, D, Ct>>::start_all(start),
        }
    }
}

// ------------------------------------------------------------------ the entries

/// What `Q` proved of the kind at the position `I` of its structure.
pub(crate) type KindPosAt<Q, I> = <<Q as Composition>::Pack as PackNth<Q, KindsOf<Q>, I>>::Out;
/// The kind at the position `I` of `Q`'s structure.
pub(crate) type DescAt<Q, I> =
    <<<Q as Composition>::Pack as PackNth<Q, KindsOf<Q>, I>>::List as KindList<Q>>::Head;
/// The full key of the nodes of the kind at `I`.
pub(crate) type FkAt<Q, I> = <DescAt<Q, I> as KindDesc<Q>>::Fk;
/// What the kind at `I` starts from.
pub(crate) type StartAt<Q, I> = StartOf<Q, DescAt<Q, I>>;
/// The kinds of the calls of the kind at `I`.
pub(crate) type CallKindsAt<Q, I> = <DescAt<Q, I> as KindDesc<Q>>::Calls;
/// The components of the kind at `I`.
pub(crate) type CompKindsAt<Q, I> = <DescAt<Q, I> as KindDesc<Q>>::Comps;
/// What `Q` proved of the calls of the kind at `I`.
pub(crate) type CallsAt<Q, I> = <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::Calls;
/// What `Q` proved of the components of the kind at `I`.
pub(crate) type CompsAt<Q, I> = <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::Comps;
/// The calls of a new node of the kind at `I` as they run, an `HList`.
pub(crate) type RunningCalls<Q, I> =
    <CallsAt<Q, I> as StartCalls<Q, DescAt<Q, I>, CallKindsAt<Q, I>>>::Running;
/// What `Q` proved of the calls of the kind at `I` from the position `J`
/// on.
pub(crate) type CallPosAt<Q, I, J> =
    <CallsAt<Q, I> as NthCall<Q, DescAt<Q, I>, CallKindsAt<Q, I>, J>>::Pos;
/// The kinds of the calls of the kind at `I` from the position `J` on.
pub(crate) type CallKindsFrom<Q, I, J> =
    <CallsAt<Q, I> as NthCall<Q, DescAt<Q, I>, CallKindsAt<Q, I>, J>>::Cs;
/// The call at the position `J` of the kind at `I` as it runs.
pub(crate) type RunningAt<Q, I, J> =
    <CallPosAt<Q, I, J> as CallList<Q, DescAt<Q, I>, CallKindsFrom<Q, I, J>>>::HeadRunning;
type CompPosAt<Q, I, J> = <CompsAt<Q, I> as NthComp<Q, DescAt<Q, I>, CompKindsAt<Q, I>, J>>::Pos;
type CompKindsFrom<Q, I, J> = <CompsAt<Q, I> as NthComp<Q, DescAt<Q, I>, CompKindsAt<Q, I>, J>>::Cs;

/// Start every call of a new node of the kind `K` (at `I`, inferred),
/// in the order of the parts, then of each part's calls, from what its
/// components left of its start.
pub(crate) fn start_calls<Q, K, I>(start: &mut StartAt<Q, I>) -> RunningCalls<Q, I>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
    CallsAt<Q, I>: StartCalls<Q, DescAt<Q, I>, CallKindsAt<Q, I>>,
{
    <CallsAt<Q, I> as StartCalls<Q, DescAt<Q, I>, CallKindsAt<Q, I>>>::start_all(start)
}

/// Lend the call of the kind `C` (at `J`, inferred) its needs at the node
/// of the kind `K` (at `I`, inferred) at `at`, and poll it once with
/// `wcx`: `Ready` when it returned, and the driver drops it. `None` when
/// the node is not in the tree.
pub(crate) fn poll_call<Q, K, C, I, J>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
    call: Pin<&mut RunningAt<Q, I, J>>,
    host: &mut CallHost<PriorityOf<Q>>,
    wcx: &mut task::Context<'_>,
) -> Option<Poll<()>>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
    CallKindsAt<Q, I>: Selector<C, J>,
    CallsAt<Q, I>: NthCall<Q, DescAt<Q, I>, CallKindsAt<Q, I>, J>,
{
    <CallPosAt<Q, I, J> as CallList<Q, DescAt<Q, I>, CallKindsFrom<Q, I, J>>>::poll_head(
        root, at, cx, call, host, wcx,
    )
}

/// Wake the component `C` of the part `O` (at `J`, inferred) at the node
/// of the kind `K` (at `I`, inferred) at `at`; `None` when the node is
/// not in the tree.
pub(crate) fn wake<Q, K, O, C, I, J>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
) -> Option<()>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
    CompKindsAt<Q, I>: Selector<super::Comp<O, C>, J>,
    CompsAt<Q, I>: NthComp<Q, DescAt<Q, I>, CompKindsAt<Q, I>, J>,
{
    <CompPosAt<Q, I, J> as CompList<Q, DescAt<Q, I>, CompKindsFrom<Q, I, J>>>::wake_head(
        root, at, cx,
    )
}

/// End the component `C` of the part `O` (at `J`, inferred) at the node
/// of the kind `K` (at `I`, inferred) at `at`, for `cause`; `None` when
/// the node is not in the tree.
pub(crate) fn end<Q, K, O, C, I, J>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
    cause: EndCause<'_, EndReasonOf<Q, DescAt<Q, I>>>,
) -> Option<()>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
    CompKindsAt<Q, I>: Selector<super::Comp<O, C>, J>,
    CompsAt<Q, I>: NthComp<Q, DescAt<Q, I>, CompKindsAt<Q, I>, J>,
{
    <CompPosAt<Q, I, J> as CompList<Q, DescAt<Q, I>, CompKindsFrom<Q, I, J>>>::end_head(
        root, at, cx, cause,
    )
}

/// The shares of a new node of the kind `K` (at `I`, inferred) at `at`:
/// every component built in the order of the parts; `None` when its
/// parent is not in the tree.
pub(crate) fn build<Q, K, I>(
    root: &mut RootNode<Q>,
    at: &<DescAt<Q, I> as KindDesc<Q>>::NewAt,
    start: &mut StartAt<Q, I>,
    args: &ArgsOf<PartsOf<Q>>,
    cx: &mut ContextOf<Q>,
) -> Option<<DescAt<Q, I> as KindDesc<Q>>::Data>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
{
    <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::build(root, at, start, args, cx)
}

/// Fire [`NodeStarted`](crate::react::NodeStarted) at the node of the kind
/// `K` (at `I`, inferred) at `at`; `None` when it is not in the tree.
pub(crate) fn node_started<Q, K, I>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
) -> Option<()>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
{
    <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::started(root, at, cx)
}

/// Fire [`NodeEnding`](crate::react::NodeEnding) at the node of the kind
/// `K` (at `I`, inferred) at `at`; `None` when it is not in the tree.
pub(crate) fn node_ending<Q, K, I>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
) -> Option<()>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
{
    <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::ending(root, at, cx)
}

//! What an edit handle's operations are made of: for each child of a
//! kind, its full key, its node, and adding, ending and moving it, chosen
//! by the child's position among the kind's children; nothing is matched
//! to a kind at run time.
use core::marker::PhantomData;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here, There},
};

use super::{ChildEditAt, EditPos, NoChildEdit};
use crate::{
    call::{
        __pack::{KindDesc, KindOps, PackIndex},
        Composition, PriorityOf,
        pack_at::{IxDesc, IxFk, IxOps, ix_table},
    },
    driver::Core,
    structure::{
        AtMostOne, Child, ChildList, ChildStorage, MoveError, NoChild, NodeKind, OccupiedError,
        SlotVec,
    },
};

/// The child kept by a storage kind under a key, as a full key: the full
/// key of its parent, `PF`, with the key appended when the storage keeps
/// children by key.
pub trait FkStep<PF>: ChildStorage {
    /// The child's full key.
    type Out: Clone + 'static;
    /// The full key of the child under `key` of the node `parent`.
    fn child(parent: &PF, key: Self::Key) -> Self::Out;
    /// The key a child with the full key `fk` is kept under.
    fn key(fk: &Self::Out) -> Self::Key;
}
impl<PF: Clone + 'static> FkStep<PF> for AtMostOne {
    type Out = PF;
    fn child(parent: &PF, _: ()) -> PF {
        parent.clone()
    }
    fn key(_: &PF) {}
}
impl<PF: FkAppend<K>, K: Clone + Eq + 'static> FkStep<PF> for SlotVec<K> {
    type Out = PF::Out;
    fn child(parent: &PF, key: K) -> PF::Out {
        parent.append(key)
    }
    fn key(fk: &PF::Out) -> K {
        PF::last(fk).clone()
    }
}

/// A full key with a key `K` appended.
pub trait FkAppend<K> {
    /// The longer full key.
    type Out: Clone + 'static;
    /// `self` with `key` appended.
    fn append(&self, key: K) -> Self::Out;
    /// The key appended last to `out`.
    fn last(out: &Self::Out) -> &K;
}
impl<K: Clone + 'static> FkAppend<K> for HNil {
    type Out = HCons<K, HNil>;
    fn append(&self, key: K) -> HCons<K, HNil> {
        HCons {
            head: key,
            tail: HNil,
        }
    }
    fn last(out: &HCons<K, HNil>) -> &K {
        &out.head
    }
}
impl<K, H: Clone + 'static, T: FkAppend<K>> FkAppend<K> for HCons<H, T> {
    type Out = HCons<H, T::Out>;
    fn append(&self, key: K) -> Self::Out {
        HCons {
            head: self.head.clone(),
            tail: self.tail.append(key),
        }
    }
    fn last(out: &Self::Out) -> &K {
        T::last(&out.tail)
    }
}

/// What an edit handle does on the children of the kind `CK` kept by
/// `S`, of a node whose full key is `PF`, under `Q`.
#[allow(private_interfaces)]
pub trait ChildOps<Q: Composition, PF, CK: NodeKind, S: ChildStorage> {
    /// A child's full key.
    type Fk: Clone + 'static;
    /// The position of a child's edit handle.
    type Edit: EditPos<CK, Q = Q, Fk = Self::Fk>;
    /// The number of the children's kind among `Q`'s kinds.
    const KIND: usize;
    /// The full key of the child under `key` of the node `parent`.
    fn fk(parent: &PF, key: S::Key) -> Self::Fk;
    /// Which child the driver keeps at `fk`.
    fn number(core: &Core<Q>, fk: &Self::Fk) -> Option<u32>;
    /// Add the child at `fk` from `start`, its calls woken at `p`.
    fn add(
        core: &mut Core<Q>,
        fk: Self::Fk,
        start: CK::Start,
        p: PriorityOf<Q>,
    ) -> Result<u32, OccupiedError>;
    /// End the child `number`, with the nodes below it, for `reason`.
    fn end(core: &mut Core<Q>, number: u32, reason: &CK::EndReason);
    /// Move the child at `from` to `to`, under another node.
    fn transplant(core: &mut Core<Q>, from: &Self::Fk, to: &Self::Fk) -> Result<(), MoveError>;
    /// The child `number` is below a node whose full key is now `parent`:
    /// give it and the nodes below it their new full keys.
    fn rekey_under(core: &mut Core<Q>, parent: &PF, number: u32);
}

/// What an edit handle does on each child of a list `L`, of a node whose
/// full key is `PF`: the first child's, then the others'.
#[allow(private_interfaces)]
pub trait ChildOpsList<Q: Composition, PF, L: ChildList> {
    /// The first child's.
    type Head: ChildOps<Q, PF, L::HeadKind, L::HeadStorage>;
    /// The others'.
    type Tail: ChildOpsList<Q, PF, L::Tail>;
    /// The child `child`, of the kind numbered `kind`, is below a node
    /// whose full key is now `parent`: give it and the nodes below it their
    /// new full keys.
    fn rekey_child(core: &mut Core<Q>, parent: &PF, child: u32, kind: usize);
}
impl<Q: Composition, PF> ChildOpsList<Q, PF, HNil> for HNil {
    type Head = NoChildOps;
    type Tail = HNil;
    #[allow(private_interfaces)]
    fn rekey_child(_: &mut Core<Q>, _: &PF, _: u32, _: usize) {}
}
impl<Q, PF, CK, S, LT, IC, T> ChildOpsList<Q, PF, HCons<Child<CK, S>, LT>>
    for HCons<ChildPos<IC>, T>
where
    Q: Composition,
    CK: NodeKind,
    S: ChildStorage,
    LT: ChildList,
    ChildPos<IC>: ChildOps<Q, PF, CK, S>,
    T: ChildOpsList<Q, PF, LT>,
{
    type Head = ChildPos<IC>;
    type Tail = T;
    #[allow(private_interfaces)]
    fn rekey_child(core: &mut Core<Q>, parent: &PF, child: u32, kind: usize) {
        if <ChildPos<IC> as ChildOps<Q, PF, CK, S>>::KIND == kind {
            <ChildPos<IC> as ChildOps<Q, PF, CK, S>>::rekey_under(core, parent, child);
        } else {
            T::rekey_child(core, parent, child, kind);
        }
    }
}

/// The child whose kind is at the position `IC` of the composition's
/// kinds.
pub struct ChildPos<IC>(PhantomData<fn() -> IC>);
impl<Q, PF, CK, S, IC> ChildOps<Q, PF, CK, S> for ChildPos<IC>
where
    Q: Composition,
    CK: NodeKind,
    S: FkStep<PF, Out = IxFk<Q, IC>>,
    IC: PackIndex,
    IxDesc<Q, IC>: KindDesc<Q, Kind = CK>,
{
    type Fk = IxFk<Q, IC>;
    type Edit = ChildEditAt<Q, IC>;
    const KIND: usize = IC::N;
    fn fk(parent: &PF, key: S::Key) -> Self::Fk {
        S::child(parent, key)
    }
    #[allow(private_interfaces)]
    fn number(core: &Core<Q>, fk: &Self::Fk) -> Option<u32> {
        <IxOps<Q, IC> as KindOps<Q, IxDesc<Q, IC>>>::ident(&core.root, fk).map(|i| i.number)
    }
    #[allow(private_interfaces)]
    fn add(
        core: &mut Core<Q>,
        fk: Self::Fk,
        start: CK::Start,
        p: PriorityOf<Q>,
    ) -> Result<u32, OccupiedError> {
        core.add_at::<IC>(fk, start, p).map_err(|_| OccupiedError)
    }
    #[allow(private_interfaces)]
    fn end(core: &mut Core<Q>, number: u32, reason: &CK::EndReason) {
        core.end_at::<IC>(number, reason);
    }
    #[allow(private_interfaces)]
    fn transplant(core: &mut Core<Q>, from: &Self::Fk, to: &Self::Fk) -> Result<(), MoveError> {
        let number =
            <Self as ChildOps<Q, PF, CK, S>>::number(core, from).ok_or(MoveError::NoSource)?;
        let parent =
            <IxOps<Q, IC> as KindOps<Q, IxDesc<Q, IC>>>::transplant(&mut core.root, from, to)?;
        core.reparent(number, parent.number);
        rekey::<Q, IC>(core, number, to.clone());
        Ok(())
    }
    #[allow(private_interfaces)]
    fn rekey_under(core: &mut Core<Q>, parent: &PF, number: u32) {
        let Some(old) = ix_table::<Q, IC>(&mut core.tables)
            .nodes
            .get(&number)
            .map(|e| S::key(&e.fk))
        else {
            return;
        };
        rekey::<Q, IC>(core, number, S::child(parent, old));
    }
}

/// Give the node `number` of the kind at `IC` the full key `fk`, and the
/// nodes below it theirs.
fn rekey<Q: Composition, IC: PackIndex>(core: &mut Core<Q>, number: u32, fk: IxFk<Q, IC>) {
    if let Some(e) = ix_table::<Q, IC>(&mut core.tables).nodes.get_mut(&number) {
        e.fk = fk.clone();
    }
    let children = core
        .nodes
        .live(number)
        .map(|n| n.children.clone())
        .unwrap_or_default();
    for c in children {
        let Some(kind) = core.nodes.live(c).map(|n| n.kind) else {
            continue;
        };
        <<IxOps<Q, IC> as KindOps<Q, IxDesc<Q, IC>>>::Children as ChildOpsList<_, _, _>>::rekey_child(
            core, &fk, c, kind,
        );
    }
}

/// The filler after the last child of a list: there is none.
pub struct NoChildOps;
impl<Q: Composition, PF> ChildOps<Q, PF, NoChild, NoChild> for NoChildOps {
    type Fk = !;
    type Edit = NoChildEdit<Q>;
    const KIND: usize = usize::MAX;
    fn fk(_: &PF, key: !) -> ! {
        key
    }
    #[allow(private_interfaces)]
    fn number(_: &Core<Q>, fk: &!) -> Option<u32> {
        match *fk {}
    }
    #[allow(private_interfaces)]
    fn add(_: &mut Core<Q>, fk: !, _: !, _: PriorityOf<Q>) -> Result<u32, OccupiedError> {
        fk
    }
    #[allow(private_interfaces)]
    fn end(_: &mut Core<Q>, _: u32, _: &()) {}
    #[allow(private_interfaces)]
    fn transplant(_: &mut Core<Q>, from: &!, _: &!) -> Result<(), MoveError> {
        match *from {}
    }
    #[allow(private_interfaces)]
    fn rekey_under(_: &mut Core<Q>, _: &PF, _: u32) {}
}

/// The operations at the position `I` of a list of operations on
/// children.
pub trait NthChildOps<Q: Composition, PF, L: ChildList, I> {
    /// They.
    type Out;
}
impl<Q: Composition, PF, L: ChildList, X: ChildOpsList<Q, PF, L>> NthChildOps<Q, PF, L, Here>
    for X
{
    type Out = X::Head;
}
impl<Q: Composition, PF, L: ChildList, X: ChildOpsList<Q, PF, L>, I> NthChildOps<Q, PF, L, There<I>>
    for X
where
    X::Tail: NthChildOps<Q, PF, L::Tail, I>,
{
    type Out = <X::Tail as NthChildOps<Q, PF, L::Tail, I>>::Out;
}

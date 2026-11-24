//! What the driver keeps of the nodes of one kind: each node's full key
//! and its calls, by node number.
use alloc::collections::BTreeMap;
use core::marker::PhantomData;

use frunk_core::hlist::HCons;

use super::Tables;
use crate::call::Composition;

/// The nodes of one kind as the driver keeps them, by node number: their
/// full keys `F` and their calls `S`.
pub struct KindTable<F, S> {
    pub(crate) nodes: BTreeMap<u32, KindEntry<F, S>>,
}
impl<F, S> Default for KindTable<F, S> {
    fn default() -> Self {
        Self {
            nodes: BTreeMap::new(),
        }
    }
}

/// One node of a kind: its full key, and its calls, each in its box until
/// it returns.
pub struct KindEntry<F, S> {
    pub(crate) fk: F,
    pub(crate) calls: S,
}

/// Reaching the tables `T` of some kinds among the tables of every kind
/// of `Q`.
pub trait TablesAt<Q: Composition, T> {
    /// They.
    fn get(tables: &mut Tables<Q>) -> &mut T;
}
/// Every table.
pub struct AllTables;
impl<Q: Composition> TablesAt<Q, Tables<Q>> for AllTables {
    fn get(tables: &mut Tables<Q>) -> &mut Tables<Q> {
        tables
    }
}
/// The tables after the first of those `L` that `A` reaches.
pub struct TailTables<A, L>(PhantomData<fn() -> (A, L)>);
impl<Q: Composition, A: TablesAt<Q, HCons<H, T>>, H: 'static, T: 'static> TablesAt<Q, T>
    for TailTables<A, HCons<H, T>>
{
    fn get(tables: &mut Tables<Q>) -> &mut T {
        &mut A::get(tables).tail
    }
}

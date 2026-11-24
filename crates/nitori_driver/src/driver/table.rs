//! What the driver keeps of the nodes of one kind: each node's full key
//! and its calls, by node number.
use alloc::collections::BTreeMap;

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

//! How the children of one kind are kept under their parent.
use alloc::vec::Vec;
use core::marker::PhantomData;

/// The place a node was to go in is taken; the node comes back.
#[derive(Debug)]
pub struct Occupied<N>(pub N);

/// A store of the children of one kind under one parent: nodes `N` told
/// apart by a key `K`, reached by a handle `H` the driver keeps.
///
/// The key is the segment of the address the child adds; the store keeps
/// it, the node does not.
pub trait Store<N, K, H>: Default {
    /// Keep `node` under `key`, or give it back when the key is taken.
    fn insert(&mut self, key: K, node: N) -> Result<H, Occupied<N>>;
    /// Take the node at `at` out.
    fn remove(&mut self, at: H) -> Option<N>;
    /// The node at `at`.
    fn get(&self, at: H) -> Option<&N>;
    /// The node at `at`, exclusive.
    fn get_mut(&mut self, at: H) -> Option<&mut N>;
    /// The key the node at `at` is kept under.
    fn key(&self, at: H) -> Option<&K>;
    /// The handle of the node kept under `key`.
    fn find(&self, key: &K) -> Option<H>;
    /// The handles of every node now kept.
    fn handles(&self) -> impl Iterator<Item = H>;
}

/// A storage kind: how many children of a kind a parent has, the key of
/// the segment of the address each adds, the handle the driver keeps, and
/// the store.
///
/// The structure chooses it for each child; the host does not replace it.
pub trait ChildStorage: 'static {
    /// The key of a child under its parent.
    type Key: Clone + Eq + 'static;
    /// What the driver keeps to reach a child again.
    type Handle: Copy + Eq + core::fmt::Debug;
    /// The store of children `N`.
    type Store<N>: Store<N, Self::Key, Self::Handle>;
}

/// The store a storage kind `S` gives for the nodes `N`.
pub type StoreOf<S, N> = <S as ChildStorage>::Store<N>;

/// At most one child: the key and the handle are `()`, the store an
/// `Option`.
///
/// For what appears and goes away over time under a node, such as a layer
/// that exists only in some phases.
pub struct AtMostOne;
impl ChildStorage for AtMostOne {
    type Key = ();
    type Handle = ();
    type Store<N> = Option<N>;
}
impl<N> Store<N, (), ()> for Option<N> {
    fn insert(&mut self, _: (), node: N) -> Result<(), Occupied<N>> {
        if self.is_some() {
            return Err(Occupied(node));
        }
        *self = Some(node);
        Ok(())
    }
    fn remove(&mut self, _: ()) -> Option<N> {
        self.take()
    }
    fn get(&self, _: ()) -> Option<&N> {
        self.as_ref()
    }
    fn get_mut(&mut self, _: ()) -> Option<&mut N> {
        self.as_mut()
    }
    fn key(&self, _: ()) -> Option<&()> {
        self.as_ref().map(|_| &())
    }
    fn find(&self, _: &()) -> Option<()> {
        self.as_ref().map(|_| ())
    }
    fn handles(&self) -> impl Iterator<Item = ()> {
        self.iter().map(|_| ())
    }
}

/// Any number of children told apart by a key `K`: the handle is the index
/// of a slot, freed slots are reused, finding a key is linear.
pub struct SlotVec<K>(PhantomData<fn() -> K>);
impl<K: Clone + Eq + 'static> ChildStorage for SlotVec<K> {
    type Key = K;
    type Handle = usize;
    type Store<N> = SlotVecStore<K, N>;
}

/// The store of [`SlotVec`]: slots holding a key and a node, or empty.
pub struct SlotVecStore<K, N> {
    pub(crate) slots: Vec<Option<(K, N)>>,
}
impl<K, N> Default for SlotVecStore<K, N> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}
impl<K: Clone + Eq, N> Store<N, K, usize> for SlotVecStore<K, N> {
    fn insert(&mut self, key: K, node: N) -> Result<usize, Occupied<N>> {
        if self.find(&key).is_some() {
            return Err(Occupied(node));
        }
        if let Some(i) = self.slots.iter().position(Option::is_none) {
            self.slots[i] = Some((key, node));
            return Ok(i);
        }
        self.slots.push(Some((key, node)));
        Ok(self.slots.len() - 1)
    }
    fn remove(&mut self, at: usize) -> Option<N> {
        self.slots.get_mut(at)?.take().map(|(_, n)| n)
    }
    fn get(&self, at: usize) -> Option<&N> {
        self.slots.get(at)?.as_ref().map(|(_, n)| n)
    }
    fn get_mut(&mut self, at: usize) -> Option<&mut N> {
        self.slots.get_mut(at)?.as_mut().map(|(_, n)| n)
    }
    fn key(&self, at: usize) -> Option<&K> {
        self.slots.get(at)?.as_ref().map(|(k, _)| k)
    }
    fn find(&self, key: &K) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|(k, _)| k == key))
    }
    fn handles(&self) -> impl Iterator<Item = usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|_| i))
    }
}

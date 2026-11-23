//! What a place does with the path of a node: why it ends, taking it out of
//! its parent's store, and its parent's place.
use core::any::TypeId;

use super::{ParentOf, PlaceKey};

use crate::{
    hooks::EndCause,
    structure::{
        Address, Append, ChildAt, ChildStorage, End, FromAddress, KeyAt, KindFor, LastChildren,
        MoveError, Node, NodePath, ParentPath, PathFrom, SplitLast, Step, Store, move_to, remove,
    },
};

/// Why a node ends when no reason of its kind's is given: the causes of
/// [`EndCause`] but its reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndBy {
    /// A node above it ended.
    Above,
    /// The last node linking to it ended.
    Unlinked,
    /// The driver was dropped.
    Dropped,
}
impl EndBy {
    /// The cause for a kind whose reason is `R`.
    pub fn cause<'a, R>(self) -> EndCause<'a, R> {
        match self {
            Self::Above => EndCause::Above,
            Self::Unlinked => EndCause::Unlinked,
            Self::Dropped => EndCause::Dropped,
        }
    }
}

/// Taking the node at a path out of its parent's store; the root is not
/// taken out this way.
pub trait Detach<R: KindFor<P>, P> {
    /// Take it out; `false` when it was not there.
    fn detach(self, root: &mut Node<R, P>) -> bool;
}
impl<R: KindFor<P>, P> Detach<R, P> for End {
    fn detach(self, _: &mut Node<R, P>) -> bool {
        false
    }
}
impl<R: KindFor<P>, P, I, H: Copy, Rr> Detach<R, P> for Step<I, H, Rr>
where
    Step<I, H, Rr>: SplitLast,
    <Step<I, H, Rr> as SplitLast>::Parent: NodePath<R, P>,
    LastChildren<R, Step<I, H, Rr>>: ChildAt<
            <Step<I, H, Rr> as SplitLast>::Index,
            P,
            Storage: ChildStorage<Handle = <Step<I, H, Rr> as SplitLast>::Handle>,
        >,
{
    fn detach(self, root: &mut Node<R, P>) -> bool {
        remove(root, self).is_some()
    }
}

/// The type of a path's parent place, as the driver keeps it.
pub trait ParentPlace {
    /// The parent's path type; none for the root.
    fn parent_place() -> Option<TypeId>;
    /// The position of the path's last step among the children of the
    /// parent's kind; none for the root.
    fn child_index() -> Option<usize>;
}
impl ParentPlace for End {
    fn parent_place() -> Option<TypeId> {
        None
    }
    fn child_index() -> Option<usize> {
        None
    }
}
impl<I, H, R> ParentPlace for Step<I, H, R>
where
    Step<I, H, R>: ParentPath<Parent: 'static> + SplitLast<Index: crate::driver::walk::Number>,
{
    fn parent_place() -> Option<TypeId> {
        Some(TypeId::of::<ParentOf<Self>>())
    }
    fn child_index() -> Option<usize> {
        Some(<<Self as SplitLast>::Index as crate::driver::walk::Number>::VALUE)
    }
}

/// The nodes at a path, reached by the driver's addresses: added under a
/// parent by a key, found by their key, moved under another parent. The
/// root is none of these.
#[allow(clippy::type_complexity)]
pub trait ChildPath<R: KindFor<P>, P>: PathFrom<R> + ParentPath + PlaceKey<R> {
    /// The parent's path, from its address.
    fn parent_path(parent: &Address<R>) -> Option<ParentOf<Self>>;
    /// Keep `node` under the parent at `parent`, under `key` (a key of
    /// this path's store); `None` when the key is taken.
    fn attach(
        root: &mut Node<R, P>,
        parent: &Address<R>,
        key: <Self as PlaceKey<R>>::Key,
        node: Node<Self::Target, P>,
    ) -> Option<Address<R>>
    where
        Self::Target: KindFor<P>;
    /// The node kept under `key` under the parent at `parent`.
    fn find_under(
        root: &Node<R, P>,
        parent: &Address<R>,
        key: &<Self as PlaceKey<R>>::Key,
    ) -> Option<Address<R>>;
    /// Move the node at `at` under the node at `to`, of its parent's place.
    fn move_under(
        root: &mut Node<R, P>,
        at: &Address<R>,
        to: &Address<R>,
    ) -> Result<Address<R>, MoveError>;
}
impl<R: KindFor<P>, P> ChildPath<R, P> for End {
    fn parent_path(_: &Address<R>) -> Option<()> {
        None
    }
    fn attach(_: &mut Node<R, P>, _: &Address<R>, _: (), _: Node<R, P>) -> Option<Address<R>>
    where
        R: KindFor<P>,
    {
        None
    }
    fn find_under(_: &Node<R, P>, _: &Address<R>, _: &()) -> Option<Address<R>> {
        None
    }
    fn move_under(
        _: &mut Node<R, P>,
        _: &Address<R>,
        _: &Address<R>,
    ) -> Result<Address<R>, MoveError> {
        Err(MoveError::NoSource)
    }
}
type ParentPathOf<X> = <X as SplitLast>::Parent;
type IndexOf<X> = <X as SplitLast>::Index;
type HandleOf<X> = <X as SplitLast>::Handle;
impl<R, P, I, H, Rr> ChildPath<R, P> for Step<I, H, Rr>
where
    R: KindFor<P>,
    Self: PlaceKey<R, Key = KeyAt<LastChildren<R, Self>, IndexOf<Self>>>,
    Self: PathFrom<R> + FromAddress<R> + SplitLast + ParentPath<Parent = ParentPathOf<Self>>,
    ParentPathOf<Self>:
        NodePath<R, P> + FromAddress<R> + Append<IndexOf<Self>, HandleOf<Self>, Out = Self> + Copy,
    LastChildren<R, Self>: ChildAt<
            IndexOf<Self>,
            P,
            Kind = <Self as PathFrom<R>>::Target,
            Storage: ChildStorage<Handle = HandleOf<Self>>,
        >,
{
    fn parent_path(parent: &Address<R>) -> Option<ParentPathOf<Self>> {
        ParentPathOf::<Self>::from_address(parent)
    }
    fn attach(
        root: &mut Node<R, P>,
        parent: &Address<R>,
        key: <Self as PlaceKey<R>>::Key,
        node: Node<Self::Target, P>,
    ) -> Option<Address<R>>
    where
        Self::Target: KindFor<P>,
    {
        let pp = ParentPathOf::<Self>::from_address(parent)?;
        let parent = pp.node_mut(root)?;
        let store =
            <LastChildren<R, Self> as ChildAt<IndexOf<Self>, P>>::store_mut(&mut parent.children);
        let h = store.insert(key, node).ok()?;
        Some(pp.append(h).address())
    }
    fn find_under(
        root: &Node<R, P>,
        parent: &Address<R>,
        key: &<Self as PlaceKey<R>>::Key,
    ) -> Option<Address<R>> {
        let pp = ParentPathOf::<Self>::from_address(parent)?;
        let parent = pp.node(root)?;
        let store = <LastChildren<R, Self> as ChildAt<IndexOf<Self>, P>>::store(&parent.children);
        Some(pp.append(store.find(key)?).address())
    }
    fn move_under(
        root: &mut Node<R, P>,
        at: &Address<R>,
        to: &Address<R>,
    ) -> Result<Address<R>, MoveError> {
        let at = Self::from_address(at).ok_or(MoveError::NoSource)?;
        let to = ParentPathOf::<Self>::from_address(to).ok_or(MoveError::NoDestination)?;
        Ok(move_to(root, at, to)?.address())
    }
}

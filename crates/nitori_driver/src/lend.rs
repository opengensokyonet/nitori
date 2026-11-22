//! Lending a node and the tree around it to a list of needs.
//!
//! The tree is split once into disjoint cells, each tagged by its place
//! ([`cells`](ChainCells)). Pass 1 plucks a cell for every exclusive need,
//! so a second exclusive need of the same place finds nothing and does not
//! compile; what is left is frozen, and pass 2 selects a cell for every
//! shared need, any number of times ([`Pass1`], [`Pass2`]). The view's type
//! follows from the list of needs and the keyed positions alone
//! ([`NeedList`]); every position is inferred where the structure and the
//! parts are concrete.
//!
//! When a need is not met, the item-by-item check ([`Locate`],
//! [`Places`]) reports which one: a need whose anchor, steps or component
//! is not on the structure, two exclusive needs of one place, or a keyed
//! need with the word that does not fit its store.
//!
//! ```
//! use frunk_core::{HList, hlist::HNil, hlist_pat};
//! use nitori_driver::{
//!     lend::lend,
//!     need::{At, Here, Kind, Mut, Ref, ToChild},
//!     structure::{AtMostOne, Child, End, Node, NodeKind, Share, insert},
//! };
//!
//! struct Root;
//! struct Layer;
//! impl NodeKind for Root {
//!     type Children = HList![Child<Layer, AtMostOne>];
//!     type Linked = HNil;
//! }
//! impl NodeKind for Layer {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! #[derive(Default)]
//! struct Count(u32);
//! #[derive(Default)]
//! struct Limit(u32);
//! struct Part;
//! impl Share<Root> for Part {
//!     type Data = HList![Count, Limit];
//! }
//! impl Share<Layer> for Part {
//!     type Data = HList![Count];
//! }
//!
//! let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! insert(&mut root, End, (), Node::<Layer, _>::new(Default::default())).ok().unwrap();
//! type Needs = HList![
//!     At<Kind<Root>, Here, Mut<Count>>,
//!     At<Kind<Root>, Here, Ref<Limit>>,
//!     At<Kind<Root>, ToChild<Layer>, Mut<Count>>,
//! ];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), |view| {
//!     let hlist_pat![count, limit, layer] = view;
//!     count.0 += limit.0 + 1;
//!     if let Some(layer) = layer {
//!         layer.0 += 1;
//!     }
//! })
//! .unwrap();
//! ```
//!
//! # What does not compile
//!
//! The examples below share a structure: a root with a layer (at most
//! one) and items by a `u32` key, each with a `Count`; the root also has
//! a `Limit`. Lending the root to the needs of every example but the
//! failing one compiles:
//!
//! ```
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![
//!     At<Kind<Root>, Here, Mut<Count>>,
//!     At<Up<0>, Here, Ref<Limit>>,
//!     At<Up<0>, ToChild<Item, ByKey<u32>>, MutList<HList![Count]>>,
//!     At<Up<0>, ToChild<Layer>, Mut<Count>>,
//! ];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! type OnItem = HList![
//!     At<Up<0>, Here, Mut<Count>>,
//!     At<Kind<Root>, ToChild<Item, ByKeyOthers<u32>>, Ref<Count>>,
//!     At<Provides<Limit>, Here, Ref<Limit>>,
//! ];
//! lend::<OnItem, _, _, _, _, _>(&mut root, item, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! // The item-by-item check, which names the need when one fails.
//! nitori_driver::lend::explain::<OnItem, Root, HList![Part], ItemAt, _, _, _, _>();
//! ```
//!
//! Two exclusive needs of one place:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![At<Kind<Root>, Here, Mut<Count>>, At<Up<0>, Here, Mut<Count>>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! ```
//!
//! An exclusive and a shared need of one place:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![At<Kind<Root>, Here, Mut<Count>>, At<Up<0>, Here, Ref<Count>>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! ```
//!
//! A component not on the structure (the layer has no `Limit`):
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![At<Kind<Root>, ToChild<Layer>, Mut<Limit>>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! ```
//!
//! Every child by key (`ByKey`) of a store on the chain; the item lent is kept in it:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![At<Kind<Root>, ToChild<Item, ByKey<u32>>, Mut<Count>>];
//! lend::<Needs, _, _, _, _, _>(&mut root, item, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! ```
//!
//! Every other child by key (`ByKeyOthers`) of a store off the chain:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![At<Up<0>, ToChild<Item, ByKeyOthers<u32>>, Mut<Count>>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! ```
//!
//! Two keyed needs of one store, even of different components:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![
//!     At<Up<0>, ToChild<Item, ByKey<u32>>, Mut<Count>>,
//!     At<Up<0>, ToChild<Item, ByKey<u32>>, MutList<HList![Count]>>,
//! ];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! ```
//!
//! Several components by key, one of them not in the children's data:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![At<Up<0>, ToChild<Item, ByKey<u32>>, MutList<HList![Count, Limit]>>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! ```
//!
//! Several components without a step by key:
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![At<Up<0>, Here, MutList<HList![Count, Limit]>>];
//! lend::<Needs, _, _, _, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! ```
//!
//! An anchor by content with two candidates on the way up (the item and the root both have `Count`):
//!
//! ```compile_fail
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     lend::lend,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, MutList, Provides, Ref, ToChild, Up},
//! #     structure::{AtMostOne, Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Layer;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>, Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # #[derive(Default)]
//! # struct Limit(u32);
//! # struct Part;
//! # impl Share<Root> for Part {
//! #     type Data = HList![Count, Limit];
//! # }
//! # impl Share<Layer> for Part {
//! #     type Data = HList![Count];
//! # }
//! # impl Share<Item> for Part {
//! #     type Data = HList![Count];
//! # }
//! # type ItemAt = Step<frunk_core::indices::There<frunk_core::indices::Here>, usize, End>;
//! # let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! # let item: ItemAt = insert(&mut root, End, 1, Node::new(Default::default())).ok().unwrap();
//! type Needs = HList![At<Provides<Count>, Here, Ref<Count>>];
//! lend::<Needs, _, _, _, _, _>(&mut root, item, &nitori_driver::event::Outbox::new(), |_| ()).unwrap();
//! ```
mod cells;
mod check;
mod keyed;
mod now;
mod pass;
mod view;
mod writes;

use core::fmt;

pub(crate) use cells::linked;
pub use cells::{
    All, Cell, ChainAtPos, ChainCells, ChildrenCells, False, IndexEq, LinkAtPos, LinkSpec, LinkTo,
    NoChain, NoLink, OffChain, OnChain, OptChildren, OptNodeCells, OptStoreCells, Others,
    RoleCells, RoleChain, RoleLink, RoleOf, RoleOther, SplitError, True, Wrap, WrapOpt,
};
pub use check::{
    Distinct, FindAll, FindCell, FindOthers, Locate, LocateIn, NoneAt, Pairwise, Places,
};
pub use keyed::{
    Keyed, KeyedCell, KeyedDesc, KeyedItem, KeyedList, KeyedMany, KeyedManyP, KeyedP, KeyedPos,
    KeyedRef, KeyedRefItem, KeyedShape, NoKeyed, NoKeyedPos, Refs, SharedKeyed,
};
pub use pass::{
    CellOf, Cx, Freeze, FreezeOne, KeyedPositions, Pass1, Pass2, RefShape, Shape, WrapManyPosition,
    WrapPosition,
};
pub use view::{
    Build, Flat, NeedList, NeedSet, TagFlat, TagFlatMany, TagFlatRef, TagNeed, TagNeedBuild,
    TagNeedRef, TagNeedRefBuild, ViewOf,
};
pub use writes::{TypeIds, Writes};

use frunk_core::hlist::{HCons, HNil};

use crate::{
    event::{Outbox, OutboxCell},
    need::ChainOf,
    structure::{KindFor, Node, NodeKind, PathFrom},
};

/// The node lent is not in the tree.
#[derive(Debug, PartialEq, Eq)]
pub enum LendError {
    /// No node at the path.
    NoNode,
}
impl fmt::Display for LendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoNode => "no node at the path lent",
        })
    }
}
impl core::error::Error for LendError {}

/// The links of the node a path `X` from `R` leads to.
pub type LinkedOf<R, X> = <<X as PathFrom<R>>::Target as NodeKind>::Linked;
/// The link the split takes, for the links of the node at `X`.
pub type LinkOf<R, P, X, IL> = <LinkedOf<R, X> as LinkSpec<R, P, X, IL>>::Lk;
/// Where the needs of the node at `X` from `R` are lent.
pub type CxOf<R, P, X> = Cx<<X as ChainOf<R, HNil>>::Chain, P>;
/// Every cell lent around the node at `X` from `R`.
///
/// The outbox comes first; the cells of the tree follow.
pub type CellsOf<'a, R, P, X, IL, T = ()> =
    HCons<OutboxCell<'a, T>, <X as ChainCells<'a, R, P, HNil, LinkOf<R, P, X, IL>>>::Out>;
/// The keyed positions of the needs `N` at `X`.
pub type PositionsOf<N, R, P, X, IK> = <N as KeyedPositions<CxOf<R, P, X>, IK>>::KP;
/// Pass 1's result for the needs `N` at `X`.
pub type Raw1Of<'a, N, R, P, X, IL, I1, T = ()> =
    <N as Pass1<'a, CxOf<R, P, X>, CellsOf<'a, R, P, X, IL, T>, I1>>::Raw;
/// What pass 1 leaves for the needs `N` at `X`.
pub type Rem1Of<'a, N, R, P, X, IL, I1, T = ()> =
    <N as Pass1<'a, CxOf<R, P, X>, CellsOf<'a, R, P, X, IL, T>, I1>>::Rem;
/// What pass 1 leaves, frozen.
pub type FrozenOf<'a, N, R, P, X, IL, I1, T = ()> =
    <Rem1Of<'a, N, R, P, X, IL, I1, T> as Freeze>::Out;
/// Pass 2's result for the needs `N` at `X`.
pub type Raw2Of<'a, N, R, P, X, IL, I1, I2, T = ()> =
    <N as Pass2<'a, CxOf<R, P, X>, FrozenOf<'a, N, R, P, X, IL, I1, T>, I2>>::Raw;

/// The list of needs `Self` lent at the path `X` from the root `R`, for
/// the parts `P`, with the instant `T` ([`Now`](crate::need::Now)); `I` is
/// every position, inferred here once.
#[diagnostic::on_unimplemented(
    message = "the needs `{Self}` cannot be lent at `{X}`",
    label = "a need is not met",
    note = "a need's anchor, steps or component is not on this structure, two needs take the same place and one is exclusive, or a keyed need uses the word that does not fit its store (`ByKey` off the chain, `ByKeyOthers` on it), or a `Now<I>` has another instant than the clock's; `explain::<Needs, Root, Parts, Path, _, _, _, _>()` names the need"
)]
pub trait LendAt<'a, R: KindFor<P>, P, X, I, T = ()>: NeedSet<P> {
    /// The keyed positions.
    type Positions: KeyedList<P, Self::Keyed>;
    /// Lend the node at `at` and the tree around it to `f`; what the view
    /// publishes goes to `outbox`, and the instant it holds is lent.
    fn lend<O>(
        root: &'a mut Node<R, P>,
        at: X,
        outbox: &'a Outbox<T>,
        f: impl FnOnce(ViewOf<'a, Self, P, Self::Positions>) -> O,
    ) -> Result<O, LendError>;
}
impl<'a, N, R, P, X, T: 'a, IL, IC, IP, IK, I1, I2> LendAt<'a, R, P, X, (IL, IC, IP, IK, I1, I2), T>
    for N
where
    R: KindFor<P>,
    X: PathFrom<R> + ChainOf<R, HNil>,
    LinkedOf<R, X>: LinkSpec<R, P, X, IL>,
    X: ChainCells<'a, R, P, HNil, LinkOf<R, P, X, IL>>,
    N: NeedSet<P>,
    N: Locate<CxOf<R, P, X>, CellsOf<'a, R, P, X, IL, T>, IC>,
    N: Places<CxOf<R, P, X>, CellsOf<'a, R, P, X, IL, T>, IP>,
    <N as Places<CxOf<R, P, X>, CellsOf<'a, R, P, X, IL, T>, IP>>::Out: Pairwise,
    N: KeyedPositions<CxOf<R, P, X>, IK>,
    PositionsOf<N, R, P, X, IK>: KeyedList<P, N::Keyed>,
    N: Pass1<'a, CxOf<R, P, X>, CellsOf<'a, R, P, X, IL, T>, I1>,
    Rem1Of<'a, N, R, P, X, IL, I1, T>: Freeze,
    N: Pass2<'a, CxOf<R, P, X>, FrozenOf<'a, N, R, P, X, IL, I1, T>, I2>,
    N: Build<
            'a,
            P,
            N::Keyed,
            PositionsOf<N, R, P, X, IK>,
            Raw1Of<'a, N, R, P, X, IL, I1, T>,
            Raw2Of<'a, N, R, P, X, IL, I1, I2, T>,
        >,
{
    type Positions = PositionsOf<N, R, P, X, IK>;
    fn lend<O>(
        root: &'a mut Node<R, P>,
        at: X,
        outbox: &'a Outbox<T>,
        f: impl FnOnce(ViewOf<'a, Self, P, Self::Positions>) -> O,
    ) -> Result<O, LendError> {
        let link = linked(<LinkedOf<R, X> as LinkSpec<R, P, X, IL>>::find(root, &at))?;
        let cells = HCons {
            head: Cell::new(outbox),
            tail: linked(at.cells(root, link))?,
        };
        let (raw1, rem) = <N as Pass1<'a, CxOf<R, P, X>, _, I1>>::take(cells);
        let frozen = rem.freeze();
        let raw2 = <N as Pass2<'a, CxOf<R, P, X>, _, I2>>::get(&frozen);
        Ok(f(
            <N as Build<'a, P, N::Keyed, Self::Positions, _, _>>::view(raw1, raw2),
        ))
    }
}

/// Lend the node at `at` and the tree around it to the needs `N`, and run
/// `f` on their view; what the view publishes ([`Publish`](crate::event::Publish))
/// goes to `outbox`.
///
/// Every position is inferred here; the type parameters after `N` are
/// written `_`. A node on the path not in the tree is an error; a linked
/// node not there breaks an invariant of the structure and panics.
pub fn lend<'a, N, R, P, X, I, T>(
    root: &'a mut Node<R, P>,
    at: X,
    outbox: &'a Outbox,
    f: impl FnOnce(ViewOf<'a, N, P, N::Positions>) -> T,
) -> Result<T, LendError>
where
    R: KindFor<P>,
    N: LendAt<'a, R, P, X, I>,
{
    N::lend(root, at, outbox, f)
}

/// The item-by-item check of the needs `N` at the path `X` from `R`, each
/// bound on its own, so that the first error names the failing need. It
/// does nothing; call it to read why a lend does not compile.
pub fn explain<N, R, P, X, IL, IC, IP, IK>()
where
    R: KindFor<P>,
    X: PathFrom<R> + ChainOf<R, HNil>,
    LinkedOf<R, X>: LinkSpec<R, P, X, IL>,
    X: ChainCells<'static, R, P, HNil, LinkOf<R, P, X, IL>>,
    N: Locate<CxOf<R, P, X>, CellsOf<'static, R, P, X, IL>, IC>,
    N: Places<CxOf<R, P, X>, CellsOf<'static, R, P, X, IL>, IP>,
    <N as Places<CxOf<R, P, X>, CellsOf<'static, R, P, X, IL>, IP>>::Out: Pairwise,
    N: KeyedPositions<CxOf<R, P, X>, IK>,
{
}

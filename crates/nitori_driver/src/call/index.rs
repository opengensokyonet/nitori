//! Positions in the kinds of a composition: the kind at one, what was
//! proved of it, and the driver's table of its nodes, reached through the
//! position alone.
//!
//! The pack names what it proved by these positions, inferred where the
//! composition is written; reaching the item through the position, never
//! through a bound on the pack, keeps the pack's proof from needing the
//! pack.
use frunk_core::indices::{Here, There};

use super::{
    Composition, KindsOf,
    pack::{KindDesc, KindList, KindOps, PackList, TableOf},
};

/// A position in a list of kinds, the first being [`Here`].
pub trait PackIndex: 'static {
    /// The position as a number, the first being 0.
    const N: usize;
    /// The kind there, in the list `L`.
    type Desc<Q: Composition, L: KindList<Q>>: KindDesc<Q>;
    /// What was proved of it, in the pack `P` of `L`.
    type Ops<Q: Composition, L: KindList<Q>, P: PackList<Q, L>>: KindOps<Q, Self::Desc<Q, L>>;
    /// The table of its nodes, among the tables of `P`.
    #[allow(clippy::type_complexity)]
    fn table<Q: Composition, L: KindList<Q>, P: PackList<Q, L>>(
        tables: &mut P::Tables,
    ) -> &mut TableOf<Q, Self::Desc<Q, L>, Self::Ops<Q, L, P>>;
}
impl PackIndex for Here {
    const N: usize = 0;
    type Desc<Q: Composition, L: KindList<Q>> = L::Head;
    type Ops<Q: Composition, L: KindList<Q>, P: PackList<Q, L>> = P::Head;
    fn table<Q: Composition, L: KindList<Q>, P: PackList<Q, L>>(
        tables: &mut P::Tables,
    ) -> &mut TableOf<Q, L::Head, P::Head> {
        P::head_table(tables)
    }
}
impl<I: PackIndex> PackIndex for There<I> {
    const N: usize = I::N + 1;
    type Desc<Q: Composition, L: KindList<Q>> = I::Desc<Q, L::Tail>;
    type Ops<Q: Composition, L: KindList<Q>, P: PackList<Q, L>> = I::Ops<Q, L::Tail, P::Tail>;
    #[allow(clippy::type_complexity)]
    fn table<Q: Composition, L: KindList<Q>, P: PackList<Q, L>>(
        tables: &mut P::Tables,
    ) -> &mut TableOf<Q, I::Desc<Q, L::Tail>, I::Ops<Q, L::Tail, P::Tail>> {
        I::table::<Q, L::Tail, P::Tail>(P::tail_tables(tables))
    }
}

/// The kind at the position `I` of `Q`'s kinds.
pub type IxDesc<Q, I> = <I as PackIndex>::Desc<Q, KindsOf<Q>>;
/// What `Q` proved of the kind at `I`.
pub type IxOps<Q, I> = <I as PackIndex>::Ops<Q, KindsOf<Q>, <Q as Composition>::Pack>;
/// The full key of the nodes of the kind at `I`.
pub type IxFk<Q, I> = <IxDesc<Q, I> as KindDesc<Q>>::Fk;
/// What a node of the kind at `I` starts from.
pub type IxStart<Q, I> = <<IxDesc<Q, I> as KindDesc<Q>>::Kind as crate::structure::NodeKind>::Start;
/// Why a node of the kind at `I` ends.
pub type IxEndReason<Q, I> =
    <<IxDesc<Q, I> as KindDesc<Q>>::Kind as crate::structure::NodeKind>::EndReason;

/// The table of the kind at `I` among `Q`'s tables.
#[allow(clippy::type_complexity)]
pub(crate) fn ix_table<Q: Composition, I: PackIndex>(
    tables: &mut <Q::Pack as PackList<Q, KindsOf<Q>>>::Tables,
) -> &mut TableOf<Q, IxDesc<Q, I>, IxOps<Q, I>> {
    I::table::<Q, KindsOf<Q>, Q::Pack>(tables)
}

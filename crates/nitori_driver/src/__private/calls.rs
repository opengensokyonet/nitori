//! The structure the examples of the calls compose, `Root { Total; [Item
//! by u32 { Count }] }`: its kinds and components, every component built
//! with nothing for any part, and a clock that stands still.
use core::time::Duration;

use crate::{
    component::{ArgsOf, Component, PartList},
    frunk_core::HList,
    id::Numbered,
    need::{Pos, View},
    structure::{Child, NodeKind, SlotVec},
};

/// The root.
#[derive(Numbered)]
pub struct Root;
impl NodeKind for Root {
    type Children = HList![Child<Item, SlotVec<u32>>];
}
/// An item, kept by number.
#[derive(Numbered)]
pub struct Item;
impl NodeKind for Item {
    type Children = HList![];
}

/// The root's total.
#[derive(Debug, Default, Numbered)]
pub struct Total(pub u32);
/// An item's count.
#[derive(Debug, Default, Numbered)]
pub struct Count(pub u32);
macro_rules! from_default {
    ($($c:ty),*) => {$(
        impl<O, L: PartList<L>, K: NodeKind> Component<O, L, K> for $c {
            fn build<P: Pos>(_: &mut K::Start, _: &ArgsOf<L>, _: View<'_, HList![], P>) -> Self {
                Self::default()
            }
        }
    )*};
}
from_default!(Total, Count);

/// A clock that stands still at zero.
#[derive(Clone, Debug)]
pub struct StillClock;
impl nitori_clock::MonotonicClock for StillClock {
    type Instant = Duration;
    type Sleep = core::future::Pending<()>;
    fn now(&self) -> Duration {
        Duration::ZERO
    }
    fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> {
        at.checked_add(by)
    }
    fn sleep_until(&self, _: Duration) -> Self::Sleep {
        core::future::pending()
    }
    fn timer_granularity(&self) -> Duration {
        Duration::ZERO
    }
}

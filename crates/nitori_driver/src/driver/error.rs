//! Why a composition cannot be driven, a node was not added, or a node
//! was not removed.
use snafu::Snafu;

use crate::{
    request::LinkedToError,
    structure::{NoParentError, OccupiedError},
};

/// Why a composition cannot be driven.
#[derive(Debug, Snafu, PartialEq, Eq)]
#[snafu(module)]
pub enum CompositionError {
    /// A place has more than 64 calls and components that act before the
    /// calls together: a node marks each in one bit of a word.
    #[snafu(display(
        "the place {place} has more than 64 calls and components that act before the calls"
    ))]
    TooManyPositions {
        /// The place's number, in tree order.
        place: usize,
    },
    /// A component's handler runs for more than 64 events: it is told
    /// them in one bit each.
    #[snafu(display("the component {component} of the place {place} handles more than 64 events"))]
    TooManySubscriptions {
        /// The place's number, in tree order.
        place: usize,
        /// The component's number at the place.
        component: usize,
    },
}

/// Why a node was not added.
#[derive(Debug, Snafu, PartialEq, Eq)]
#[snafu(module)]
pub enum AddError {
    /// The root ended.
    #[snafu(display("the root ended"))]
    Ended,
    /// No node at the parent's path.
    #[snafu(display("the node was not added"))]
    NoParent {
        /// Why.
        source: NoParentError,
    },
    /// The parent has a child of this kind under the same key.
    #[snafu(display("the node was not added"))]
    Occupied {
        /// Why.
        source: OccupiedError,
    },
}
impl AddError {
    pub(crate) fn no_parent() -> Self {
        Self::NoParent {
            source: NoParentError,
        }
    }
    pub(crate) fn occupied() -> Self {
        Self::Occupied {
            source: OccupiedError,
        }
    }
}

/// Why [`Driver::remove`](crate::driver::Driver::remove) removed no node.
#[derive(Debug, Snafu, PartialEq, Eq)]
#[snafu(module)]
pub enum RemoveError {
    /// No node at the path, or the driver ended.
    #[snafu(display("no node at the path to remove"))]
    NoNode,
    /// Nodes link to it.
    #[snafu(display("the node was not removed"))]
    LinkedTo {
        /// Why.
        source: LinkedToError,
    },
}

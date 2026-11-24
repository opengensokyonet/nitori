//! Which rows of a kind's table a column takes: the one on the route to
//! the lend point, or every one the walk visited.
//!
//! The walk visits the route to the lend point and, below each anchor on
//! it, the nodes the plan wants there, so one kind's table may hold the
//! row on the route and the rows of the members of an `Under`. Every node
//! a column takes from is on one chain from the root through the members
//! around it, and every anchor around the column is on that chain too. A
//! node of the chain no deeper than every anchor around is so the one on
//! the route: a column of a kind that deep takes that row alone, and a
//! node set below an anchor that deep takes the rows below the anchor's
//! node on the route alone ([`LineAt`]). The union of a `+`, and of the
//! reactions to a point, no longer says which anchor its items are below,
//! so it is taken as if inside the root ([`Whole`](super::plus::Whole)):
//! every row of a kind below the root.
use core::{marker::PhantomData, ops::Range};

use frunk_core::hlist::{HCons, HNil};

use super::rows::{self, Span};

/// The depth of the kind at a route: how many levels below the root.
pub trait RouteDepth {
    /// The depth.
    const DEPTH: usize;
}
impl RouteDepth for HNil {
    const DEPTH: usize = 0;
}
impl<H, T: RouteDepth> RouteDepth for HCons<H, T> {
    const DEPTH: usize = T::DEPTH + 1;
}

/// Where a part of a plan is taken, as a type, so that which rows a column
/// takes is known when it is compiled.
pub trait LineAt {
    /// The depth of the shallowest anchor around, none at the lend point
    /// itself.
    const ANCHOR: Option<usize>;
}
/// At the lend point: every column is on the route.
pub struct AtPoint;
impl LineAt for AtPoint {
    const ANCHOR: Option<usize> = None;
}
/// Inside the anchor of the kind at the route `A` as well as where `Ln`
/// says; inside the root for the union of a `+`, whose items no longer say
/// which anchor they are below.
pub struct Inside<Ln, A>(PhantomData<fn() -> (Ln, A)>);
impl<Ln: LineAt, A: RouteDepth> LineAt for Inside<Ln, A> {
    const ANCHOR: Option<usize> = match Ln::ANCHOR {
        Some(a) if a < A::DEPTH => Some(a),
        _ => Some(A::DEPTH),
    };
}

/// Which of one kind's `len` rows to take, where `Ln` says, for a column of
/// that kind at the depth `depth`, when not every one: the row of the node
/// on the route, at `route` among them.
#[inline]
pub fn route_rows<Ln: LineAt>(
    depth: usize,
    route: Option<usize>,
    len: usize,
) -> Option<Range<usize>> {
    // Off the route, or one row at most, the common case along one route:
    // nothing to pick out.
    if off::<Ln>(depth) || len <= 1 {
        return None;
    }
    // A route the walk did not go through, which the needs' types rule out:
    // every row.
    debug_assert!(
        route.is_some(),
        "a column no deeper than every anchor around has its node on the route"
    );
    route.map(|i| i..i + 1)
}

/// Which of `rows`, one kind's rows in the order of the walk, to take,
/// where `Ln` says, for a node set below an anchor at the depth `depth`,
/// when not every one: those below the anchor's node on the route, at
/// `anchor` in the walk.
#[inline]
pub fn anchor_rows<Ln: LineAt, T>(
    depth: usize,
    anchor: Option<Span>,
    rows: &[T],
    span: impl Fn(&T) -> Span,
) -> Option<Range<usize>> {
    // Below the root, every row is below it; one row at most, the keys pick
    // it out if it is below another anchor ([`Collect`](super::tree::Collect)).
    if depth == 0 || off::<Ln>(depth) || rows.len() <= 1 {
        return None;
    }
    anchor.map(|on| rows::member_rows_of(rows, span, on))
}

/// Whether a kind at the depth `depth` is deeper than an anchor around,
/// where `Ln` says.
const fn off<Ln: LineAt>(depth: usize) -> bool {
    match Ln::ANCHOR {
        Some(a) => depth > a,
        None => false,
    }
}

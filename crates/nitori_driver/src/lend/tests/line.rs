//! Which rows of a kind's table a column or a node set takes, where the
//! anchors around it say: the shallowest anchor around, and the rows of
//! the route or below the anchor's node on it.
use frunk_core::hlist::{HCons, HNil};

use crate::lend::{
    line::{AtPoint, Inside, LineAt, anchor_rows, route_rows},
    rows::Span,
};

/// Routes one, two and three levels below the root.
type D1 = HCons<(), HNil>;
type D2 = HCons<(), D1>;
type D3 = HCons<(), D2>;

#[test]
fn the_shallowest_anchor_around() {
    assert_eq!(<AtPoint as LineAt>::ANCHOR, None);
    assert_eq!(<Inside<AtPoint, D2> as LineAt>::ANCHOR, Some(2));
    // An anchor inside shallower than the one outside.
    assert_eq!(<Inside<Inside<AtPoint, D2>, D1> as LineAt>::ANCHOR, Some(1));
    // An anchor inside deeper than the one outside.
    assert_eq!(<Inside<Inside<AtPoint, D1>, D3> as LineAt>::ANCHOR, Some(1));
    assert_eq!(
        <Inside<Inside<Inside<AtPoint, D3>, D1>, D2> as LineAt>::ANCHOR,
        Some(1)
    );
    // The union of a `+`, inside the root.
    assert_eq!(
        <Inside<Inside<AtPoint, D2>, HNil> as LineAt>::ANCHOR,
        Some(0)
    );
}

#[test]
fn a_column_no_deeper_than_the_anchors_takes_the_row_on_the_route() {
    assert_eq!(route_rows::<AtPoint>(2, Some(1), 3), Some(1..2));
    assert_eq!(route_rows::<Inside<AtPoint, D2>>(2, Some(0), 3), Some(0..1));
    assert_eq!(route_rows::<Inside<AtPoint, D2>>(1, Some(2), 3), Some(2..3));
    // Deeper than an anchor around: every row.
    assert_eq!(route_rows::<Inside<AtPoint, D1>>(2, Some(0), 3), None);
    assert_eq!(
        route_rows::<Inside<Inside<AtPoint, D3>, D1>>(2, Some(0), 3),
        None
    );
    // In the union of a `+`: every row below the root.
    assert_eq!(route_rows::<Inside<AtPoint, HNil>>(1, Some(0), 3), None);
    // One row: nothing to pick out.
    assert_eq!(route_rows::<AtPoint>(2, Some(0), 1), None);
}

#[test]
fn a_node_set_below_an_anchor_on_the_route_takes_that_part() {
    let span = |start, end| Span { start, end };
    // Three nodes of one kind in the order of the walk, two below the
    // anchor's node on the route, one below another node of its kind.
    let rows = [span(2, 3), span(4, 5), span(7, 8)];
    let on = Some(span(1, 6));
    assert_eq!(
        anchor_rows::<Inside<AtPoint, D1>, _>(1, on, &rows, |s| *s),
        Some(0..2)
    );
    // Inside a shallower anchor: every row, picked out by key.
    assert_eq!(
        anchor_rows::<Inside<Inside<AtPoint, D1>, D2>, _>(2, on, &rows, |s| *s),
        None
    );
    // Below the root: every row is below it.
    assert_eq!(
        anchor_rows::<Inside<AtPoint, HNil>, _>(0, on, &rows, |s| *s),
        None
    );
    // In the union of a `+`: every row.
    assert_eq!(
        anchor_rows::<Inside<Inside<AtPoint, D1>, HNil>, _>(1, on, &rows, |s| *s),
        None
    );
}

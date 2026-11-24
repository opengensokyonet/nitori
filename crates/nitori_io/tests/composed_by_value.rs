//! An access composed by value: a function wraps the access it is given, and
//! `into_parts` returns that access to read on after the composition.
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{Composed, ReceiverAccess, call};
use nitori_io::{adapters::ReadLimit, calls::ReadArrayError, prelude::*};

/// Wraps the access it is given, as it is, in a read budget.
fn limit<I: ReceiverAccess>(io: I, budget: usize) -> Composed<I, ReadLimit<usize>> {
    Composed::new(io, ReadLimit(budget))
}

type Reads<E> = (
    Result<[u8; 2], ReadArrayError<E>>,
    usize,
    Result<[u8; 1], ReadArrayError<E>>,
);

/// Reads two bytes within a budget of one, then one byte from the parent
/// access that `into_parts` returns.
#[call(sync)]
async fn read_within_then_after<F: Read>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Reads<F::Error> {
    let mut limited = limit(io, 1);
    let within = limited.read_array::<2>().await;
    let (io, ReadLimit(left)) = limited.into_parts();
    let after = io.read_array::<1>().await;
    (within, left, after)
}

#[test]
fn into_parts_returns_the_parent_to_read_on() {
    let mut input = b"abc".as_slice();
    let (within, left, after) = input.sync_read_within_then_after_unpin();
    assert_eq!(within.unwrap_err().completed(), 1);
    assert_eq!(left, 0);
    assert_eq!(after.unwrap(), *b"b");
    assert_eq!(input, b"c");
}

type Nested<E> = (
    Result<[u8; 2], ReadArrayError<E>>,
    Result<[u8; 1], ReadArrayError<E>>,
    Result<[u8; 1], ReadArrayError<E>>,
    [usize; 3],
);

/// Composes a budget of one on a budget of three taken by value, then a
/// budget of one by value on that, and returns the reads through the
/// borrowed inner budget, the nested budget and the outer budget, with what
/// each budget has left.
#[call(sync)]
async fn compose_on_composed<F: Read>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Nested<F::Error> {
    let mut outer = limit(io, 3);
    let mut inner = outer.compose(ReadLimit(1usize));
    let first = inner.read_array::<2>().await;
    let inner_left = inner.into_state().0;
    let mut nested = limit(outer, 1);
    let second = nested.read_array::<1>().await;
    let (mut outer, ReadLimit(nested_left)) = nested.into_parts();
    let third = outer.read_array::<1>().await;
    let outer_left = outer.into_state().0;
    (first, second, third, [inner_left, nested_left, outer_left])
}

#[test]
fn a_composed_access_composes_again() {
    let mut input = b"abcd".as_slice();
    let (first, second, third, left) = input.sync_compose_on_composed_unpin();
    assert_eq!(first.unwrap_err().completed(), 1);
    assert_eq!(second.unwrap(), *b"b");
    assert_eq!(third.unwrap(), *b"c");
    assert_eq!(left, [0, 0, 0]);
    assert_eq!(input, b"d");
}

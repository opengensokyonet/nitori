//! Merging the attributes of consecutive segments.

/// The metadata or report of consecutive segments merged into the one value
/// for all of them, so that an operation made of several reads or writes
/// returns their attributes instead of dropping them. A 0-RTT mark, for
/// example, merges to "some of these bytes arrived in 0-RTT".
///
/// The default value is what no segments merge to: combining it with any
/// value, on either side, gives that value. Operations that may complete
/// without a segment, such as reading zero bytes, return it. A type without
/// such a value can be given one, such as a variant for no segments.
pub trait Combine: Default {
    /// Merge `self`, which came first, with `next`.
    #[must_use]
    fn combine(self, next: Self) -> Self;
}

/// No attributes, merged into none.
impl Combine for () {
    fn combine(self, (): ()) {}
}

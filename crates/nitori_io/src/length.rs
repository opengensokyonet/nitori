//! Arithmetic used when bounding one byte length by another.

/// A source that never reaches normal EOF, or a budget with no upper bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Infinite;

/// The mathematical minimum of two lengths, taking both operands by value.
/// For the built-in unsigned types a finite right operand determines the
/// result type. An infinite right operand preserves the left operand's type.
pub trait Min<Rhs = Self> {
    /// Representation of the smaller value.
    type Output;
    /// Return the smaller length.
    fn min(self, rhs: Rhs) -> Self::Output;
}

macro_rules! finite_min {
    ($lhs:ty; $($rhs:ty),+) => {$(
        impl Min<$rhs> for $lhs {
            type Output = $rhs;
            fn min(self, rhs: $rhs) -> $rhs {
                // For unsigned integers the only failed conversion is a value
                // above the largest possible rhs.
                <$rhs>::try_from(self).map_or(rhs, |lhs| core::cmp::min(lhs, rhs))
            }
        }
    )+};
}
macro_rules! lengths {
    ($($integer:ty),+) => {$(
        finite_min!($integer; u8, u16, u32, u64, u128, usize);
        impl Min<Infinite> for $integer {
            type Output = Self;
            fn min(self, _: Infinite) -> Self { self }
        }
        impl Min<$integer> for Infinite {
            type Output = $integer;
            fn min(self, rhs: $integer) -> $integer { rhs }
        }
        impl core::ops::SubAssign<$integer> for Infinite {
            fn sub_assign(&mut self, _: $integer) {}
        }
    )+};
}
lengths!(u8, u16, u32, u64, u128, usize);

impl Min for Infinite {
    type Output = Self;
    fn min(self, _: Self) -> Self {
        self
    }
}

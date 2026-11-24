//! Type-level numbers: what tells two components apart where a need is
//! proved.
//!
//! A need finds the node it takes from by the components the nodes have,
//! and says why when no node or more than one has the component. Counting
//! needs telling component types apart by trait resolution, so every
//! component has a number ([`Numbered`]), written as a list of bits
//! ([`BCons`], [`B0`], [`B1`], [`BNil`]), the highest first. The derive
//! gives the number; a generic type mixes in the numbers of its type
//! parameters ([`Mixed`]).
//!
//! ```
//! use nitori_driver::id::{False, Numbered, NumberEq, True};
//!
//! #[derive(Numbered)]
//! struct Credit;
//! #[derive(Numbered)]
//! struct Keys;
//! #[derive(Numbered)]
//! struct Pair<A, B>(A, B);
//!
//! fn same<A: Numbered, B: Numbered>()
//! where
//!     A::Number: NumberEq<B::Number, Out = True>,
//! {
//! }
//! fn differ<A: Numbered, B: Numbered>()
//! where
//!     A::Number: NumberEq<B::Number, Out = False>,
//! {
//! }
//! same::<Pair<Credit, Keys>, Pair<Credit, Keys>>();
//! differ::<Pair<Credit, Keys>, Pair<Keys, Credit>>();
//! differ::<Credit, Keys>();
//! ```
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

/// The type-level number of a type: `#[derive(Numbered)]` for a
/// component, and for every type a generic component is generic over.
pub use nitori_driver_macros::Numbered;

/// A type with a type-level number, a list of bits the highest first.
///
/// Two types of one crate never have the same number: the derive refuses
/// the second, and tells apart the types derived in the body of a
/// `macro_rules!` macro by the places it is expanded. Across crates two numbers are the same by chance about once
/// in 2⁶⁴, which nothing checks.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no type-level number: derive `Numbered`",
    label = "`{Self}` is not `#[derive(Numbered)]`",
    note = "every node kind of a composition, every component and every reaction point is told apart by its number; `#[derive(Numbered)]` gives it one"
)]
pub trait Numbered {
    /// The number: [`BCons`] of [`B0`] and [`B1`], ending in [`BNil`].
    type Number;
}

/// A list of bits as the number it spells, the highest bit first: what
/// a component is told apart by while the driver runs.
///
/// What the driver reads numbers with; not for use otherwise.
#[doc(hidden)]
pub trait NumberValue {
    /// How many bits the list has.
    const LEN: u32;
    /// The number.
    const VALUE: u64;
}
impl NumberValue for BNil {
    const LEN: u32 = 0;
    const VALUE: u64 = 0;
}
impl<T: NumberValue> NumberValue for BCons<B0, T> {
    const LEN: u32 = T::LEN + 1;
    const VALUE: u64 = T::VALUE;
}
impl<T: NumberValue> NumberValue for BCons<B1, T> {
    const LEN: u32 = T::LEN + 1;
    const VALUE: u64 = (1 << T::LEN) | T::VALUE;
}
/// The number of `X` as a value.
pub(crate) const fn number_of<X: Numbered<Number: NumberValue>>() -> u64 {
    <X::Number as NumberValue>::VALUE
}

/// The bit 0.
pub struct B0;
/// The bit 1.
pub struct B1;
/// The end of a list of bits.
pub struct BNil;
/// A bit `H`, then the lower bits `T`.
pub struct BCons<H, T>(PhantomData<fn() -> (H, T)>);

/// The answer yes, as a type.
pub struct True;
/// The answer no, as a type.
pub struct False;

/// Whether two lists of bits are the same: [`True`] or [`False`]. The
/// highest bits are compared first, and the first that differs decides.
pub trait NumberEq<B> {
    /// [`True`] or [`False`].
    type Out;
}
impl NumberEq<BNil> for BNil {
    type Out = True;
}
impl<T: NumberEq<U>, U> NumberEq<BCons<B0, U>> for BCons<B0, T> {
    type Out = T::Out;
}
impl<T: NumberEq<U>, U> NumberEq<BCons<B1, U>> for BCons<B1, T> {
    type Out = T::Out;
}
impl<T, U> NumberEq<BCons<B1, U>> for BCons<B0, T> {
    type Out = False;
}
impl<T, U> NumberEq<BCons<B0, U>> for BCons<B1, T> {
    type Out = False;
}

/// Two lists of bits of one length, bit by bit exclusive-or.
///
/// What the derive mixes numbers with; not for use otherwise.
#[doc(hidden)]
pub trait Xor<B> {
    /// The list of the bits that differ.
    type Out;
}
impl Xor<BNil> for BNil {
    type Out = BNil;
}
impl<T: Xor<U>, U> Xor<BCons<B0, U>> for BCons<B0, T> {
    type Out = BCons<B0, T::Out>;
}
impl<T: Xor<U>, U> Xor<BCons<B1, U>> for BCons<B1, T> {
    type Out = BCons<B0, T::Out>;
}
impl<T: Xor<U>, U> Xor<BCons<B1, U>> for BCons<B0, T> {
    type Out = BCons<B1, T::Out>;
}
impl<T: Xor<U>, U> Xor<BCons<B0, U>> for BCons<B1, T> {
    type Out = BCons<B1, T::Out>;
}

/// A list of bits rotated left by one: the highest bit becomes the lowest.
#[doc(hidden)]
pub trait RotateLeft {
    /// The rotated list.
    type Out;
}
impl<H, T: PushBit<H>> RotateLeft for BCons<H, T> {
    type Out = T::Out;
}
/// A list of bits with the bit `H` added as the lowest.
#[doc(hidden)]
pub trait PushBit<H> {
    /// The longer list.
    type Out;
}
impl<H> PushBit<H> for BNil {
    type Out = BCons<H, BNil>;
}
impl<H, B, T: PushBit<H>> PushBit<H> for BCons<B, T> {
    type Out = BCons<B, T::Out>;
}

/// Two lists of bits of one length, bit by bit and.
#[doc(hidden)]
pub trait And<B> {
    /// The list of the bits set in both.
    type Out;
}
impl And<BNil> for BNil {
    type Out = BNil;
}
impl<T: And<U>, U> And<BCons<B0, U>> for BCons<B0, T> {
    type Out = BCons<B0, T::Out>;
}
impl<T: And<U>, U> And<BCons<B1, U>> for BCons<B1, T> {
    type Out = BCons<B1, T::Out>;
}
impl<T: And<U>, U> And<BCons<B1, U>> for BCons<B0, T> {
    type Out = BCons<B0, T::Out>;
}
impl<T: And<U>, U> And<BCons<B0, U>> for BCons<B1, T> {
    type Out = BCons<B0, T::Out>;
}

/// A list of bits `x` mixed with itself, not linearly:
/// `x ⊕ (rotate₁(x) ∧ rotate₂(x))`.
///
/// Exclusive-or and rotation alone are linear, so the parameters of a
/// nested generic type could cancel out: `Pair<Wrap<A>, A>` would have
/// one number whatever `A` is. The and keeps them from it.
///
/// It is not one to one (all zeros and all ones both give zeros, for
/// one), but exhausted at 12, 16 and 20 bits, two inputs give one output
/// less often than under a random function, and less often the wider the
/// bits; at 64 bits that is negligible beside the chance of two numbers
/// alike across crates.
#[doc(hidden)]
pub trait Scramble {
    /// The mixed list.
    type Out;
}
impl<N, R1, R2> Scramble for N
where
    N: RotateLeft<Out = R1>,
    R1: RotateLeft<Out = R2> + And<R2>,
    N: Xor<<R1 as And<R2>>::Out>,
{
    type Out = <N as Xor<<R1 as And<R2>>::Out>>::Out;
}

/// The numbers of the type parameters of a generic type, one per
/// position, as they are mixed into its own number: each scrambled
/// ([`Scramble`]), the first then rotated by one, the second by two, and
/// so on, then all of them exclusive-or.
///
/// What the derive mixes numbers with; not for use otherwise.
#[doc(hidden)]
pub trait MixParams {
    /// The mixed numbers.
    type Out;
}
impl<N: Scramble> MixParams for HCons<N, HNil>
where
    N::Out: RotateLeft,
{
    type Out = <N::Out as RotateLeft>::Out;
}
impl<N, M, T> MixParams for HCons<N, HCons<M, T>>
where
    HCons<M, T>: MixParams,
    N: Scramble,
    N::Out: Xor<<HCons<M, T> as MixParams>::Out>,
    <N::Out as Xor<<HCons<M, T> as MixParams>::Out>>::Out: RotateLeft,
{
    type Out = <<N::Out as Xor<<HCons<M, T> as MixParams>::Out>>::Out as RotateLeft>::Out;
}

/// The number of a generic type: its own number `Own` exclusive-or the
/// numbers of its type parameters `Params` (an `HList`), each scrambled
/// and rotated by its position ([`MixParams`]).
///
/// Rotating keeps the parameters' places apart, so `Pair<A, B>` and
/// `Pair<B, A>` differ, and so do `Wrap<Wrap<X>>` and `X`; scrambling
/// keeps a parameter that comes in twice from cancelling out, so
/// `Pair<Wrap<A>, A>` and `Pair<Wrap<B>, B>` differ.
pub type Mixed<Own, Params> = <Own as Xor<<Params as MixParams>::Out>>::Out;

#[cfg(test)]
mod tests;

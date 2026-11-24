use core::marker::PhantomData;

use super::{False, NumberEq, Numbered, True};

#[derive(Numbered)]
struct Credit;
#[derive(Numbered)]
struct Keys;
#[derive(Numbered)]
struct Wrap<C>(PhantomData<C>);
#[derive(Numbered)]
struct Pair<A, B>(PhantomData<(A, B)>);

fn same<A: Numbered, B: Numbered>()
where
    A::Number: NumberEq<B::Number, Out = True>,
{
}
fn differ<A: Numbered, B: Numbered>()
where
    A::Number: NumberEq<B::Number, Out = False>,
{
}

/// A component derived in the body of a `macro_rules!` macro, which has its
/// name where the macro is defined.
macro_rules! component {
    () => {
        #[derive(Numbered)]
        pub struct Expanded;
    };
}
mod first {
    use super::Numbered;
    component!();
}
mod second {
    use super::Numbered;
    component!();
}

#[test]
fn expansions_of_one_macro_differ() {
    differ::<first::Expanded, second::Expanded>();
    same::<first::Expanded, first::Expanded>();
}

#[test]
fn derived_numbers_tell_types_apart() {
    same::<Credit, Credit>();
    differ::<Credit, Keys>();
    same::<Pair<Credit, Keys>, Pair<Credit, Keys>>();
    differ::<Pair<Credit, Keys>, Pair<Keys, Credit>>();
    differ::<Wrap<Wrap<Credit>>, Credit>();
    differ::<Wrap<Credit>, Wrap<Keys>>();
    differ::<Wrap<Credit>, Credit>();
    differ::<Pair<Credit, Credit>, Pair<Keys, Keys>>();
}

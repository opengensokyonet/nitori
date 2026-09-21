use std::{cell::Cell, pin::Pin};

struct Inner<'host> {
    value: Cell<&'host mut u8>,
}

struct Outer<'visit> {
    inner: Pin<&'visit mut Inner<'visit>>,
    state: &'visit mut u8,
}

// This fails without GATs too: the required lifetime substitution is not legal
// for arbitrary author-defined inner hosts.
fn compose<'access, 'host: 'access>(
    inner: Pin<&'access mut Inner<'host>>,
    state: &'access mut u8,
) -> Outer<'access> {
    Outer { inner, state }
}

fn main() {}

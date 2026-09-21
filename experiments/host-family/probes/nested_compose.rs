use host_family::Family;
use std::{marker::PhantomData, pin::Pin};

struct Layer<'visit, F: Family + 'visit> {
    inner: Pin<&'visit mut F::Host<'visit>>,
    state: &'visit mut u8,
}

struct Layered<F>(PhantomData<F>);
impl<F: Family> Family for Layered<F> {
    type Host<'visit> = Layer<'visit, F> where Self: 'visit;
}

// A child on an arbitrary family tries to add its own local state. The outer
// access lifetime may be shorter than the existing host's inner lifetime.
fn compose<'access, 'host, F: Family + 'host>(
    host: Pin<&'access mut F::Host<'host>>,
    state: &'access mut u8,
) -> <Layered<F> as Family>::Host<'access>
where
    'host: 'access,
{
    Layer { inner: host, state }
}

fn main() {}

#![allow(dead_code)]
use host_family::{Family, Fixed};
use std::pin::Pin;

struct Layer<'visit, F: Family + 'visit> {
    inner: Pin<&'visit mut F::Host<'visit>>,
    state: &'visit mut u8,
}

fn compose<'access, H: 'access>(
    inner: Pin<&'access mut H>,
    state: &'access mut u8,
) -> Layer<'access, Fixed<H>> {
    Layer { inner, state }
}

fn main() {
    let text = String::from("non-static");
    let mut host = text.as_str();
    let mut state = 0;
    let layer = compose(Pin::new(&mut host), &mut state);
    assert_eq!(*layer.inner, "non-static");
}

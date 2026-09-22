use std::pin::Pin;

// This view is covariant in its only lifetime when passed by value.
struct Covariant<'data>(&'data str);

fn owned<'short, 'long: 'short>(value: Covariant<'long>) -> Covariant<'short> {
    value
}

// Covariance of the pointee does not permit changing its type behind &mut.
fn borrowed<'short, 'long: 'short>(
    value: Pin<&'short mut Covariant<'long>>,
) -> Pin<&'short mut Covariant<'short>> {
    value
}

fn main() {}

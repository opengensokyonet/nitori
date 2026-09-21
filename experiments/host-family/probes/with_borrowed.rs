use host_family::{Family, Fixed};
use std::pin::Pin;

fn with<F: Family, B, R>(body: B) -> B
where
    B: for<'host> FnOnce(Pin<&mut F::Host<'host>>) -> R,
{
    body
}

fn borrowed<'data>(value: &'data str) {
    let body = with::<Fixed<&'data str>, _, usize>(|host| host.len());
    let mut host = value;
    assert_eq!(body(Pin::new(&mut host)), value.len());
}

fn main() { let text = String::from("borrowed"); borrowed(&text); }

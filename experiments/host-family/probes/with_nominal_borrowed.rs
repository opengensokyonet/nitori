#![feature(coroutine_trait)]
use host_family::{Call, lending::{Root, with}};
use std::{ops::CoroutineState::Complete, pin::{Pin, pin}, task::{Context, Poll, Waker}};

fn borrowed<'data>(text: &'data str) -> &'data str {
    let mut host = text;
    let mut view = Pin::new(&mut host);
    let mut captured = 0;
    let output = {
        let mut request = pin!(with::<Root<&'data str>, _, _>(|access| {
            captured += 1;
            **access.into_pin()
        }));
        match request.as_mut().poll(Pin::new(&mut view), &mut Context::from_waker(Waker::noop())) {
            Poll::Ready(Complete(value)) => value,
            _ => unreachable!(),
        }
    };
    assert_eq!(captured, 1);
    output
}

fn main() { let text = String::from("borrowed"); assert_eq!(borrowed(&text), text); }

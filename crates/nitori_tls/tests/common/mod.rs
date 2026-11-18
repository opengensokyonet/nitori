//! Helpers shared by the integration tests.

#[allow(dead_code)]
pub mod backend;
#[allow(dead_code)]
pub mod pki;
pub mod rfc8448;
#[allow(dead_code)]
pub mod steps;
#[allow(dead_code)]
pub mod streams;

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use std::time::Duration;
use time::UtcDateTime;

/// 2026-11-04T00:00:00Z.
#[allow(dead_code)]
pub fn now() -> UtcDateTime {
    UtcDateTime::UNIX_EPOCH + Duration::from_secs(1_793_750_400)
}

/// The output of a future that is ready at once.
#[allow(dead_code)]
pub fn ready<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("future not ready"),
    }
}

/// The bytes of a hexadecimal string.
#[allow(dead_code)]
pub fn hex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

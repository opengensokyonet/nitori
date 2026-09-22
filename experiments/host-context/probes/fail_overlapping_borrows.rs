mod protocol;

use std::pin::Pin;
use std::task::{Context, Poll};

use protocol::{HostContext, StoredContext};

fn overlap(context: &mut StoredContext<'_, '_>, cx: &mut Context<'_>) {
    let first = Pin::new(&mut *context).poll_view(cx);
    let second = Pin::new(&mut *context).poll_view(cx);
    // Both borrows remain live through the second poll.
    if let (Poll::Ready(first), Poll::Ready(second)) = (first, second) {
        assert_eq!(first.visits.get(), second.visits.get());
    }
}

fn main() {}

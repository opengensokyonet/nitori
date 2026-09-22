mod protocol;

use std::pin::Pin;
use std::task::{Context, Poll};

use protocol::{HostContext, StoredContext, View};

fn escape<'view, 'owner: 'view>(
    initial: View<'view, 'owner>,
    cx: &mut Context<'_>,
) -> Pin<&'view mut View<'view, 'owner>> {
    let mut context = StoredContext::new(initial);
    match Pin::new(&mut context).poll_view(cx) {
        Poll::Ready(view) => view,
        Poll::Pending => panic!("stored view must be ready"),
    }
}

fn main() {}

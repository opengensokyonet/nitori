mod protocol;

use std::pin::Pin;
use std::task::{Context, Poll};

use protocol::{HostContext, StoredContext, View};

struct ProjectedContext<'view, 'owner> {
    parent: StoredContext<'view, 'owner>,
    cached: Option<Pin<&'view mut View<'view, 'owner>>>,
}

impl<'view, 'owner: 'view> ProjectedContext<'view, 'owner> {
    fn initialize(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let borrowed = std::task::ready!(Pin::new(&mut self.parent).poll_view(cx));
        // This would store a borrow of self.parent back inside self.
        self.cached = Some(borrowed);
        Poll::Ready(())
    }
}

fn main() {}

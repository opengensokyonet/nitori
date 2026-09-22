mod protocol;

use std::pin::Pin;
use std::task::{Context, Poll};

use protocol::{HostContext, StoredContext, View};

struct ProjectedContext<'round, 'view, 'owner> {
    parent: Pin<&'round mut StoredContext<'view, 'owner>>,
    cached: Option<Pin<&'round mut View<'view, 'owner>>>,
}

impl<'round, 'view: 'round, 'owner: 'view> ProjectedContext<'round, 'view, 'owner> {
    fn initialize(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let borrowed = std::task::ready!(self.parent.as_mut().poll_view(cx));
        // Even though parent lives outside this object, this reborrow only lasts
        // as long as the current mutable borrow of self, not all of 'round.
        self.cached = Some(borrowed);
        Poll::Ready(())
    }
}

fn main() {}

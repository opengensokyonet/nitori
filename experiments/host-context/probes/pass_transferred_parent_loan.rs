mod protocol;

use std::pin::{Pin, pin};
use std::task::{Context, Poll, Waker, ready};

use protocol::{HostContext, StoredContext, View};

// Compare fail_cached_borrowed_parent: the parent still lives outside this
// object, but initialization consumes its original loan after checking readiness.
struct CachedParent<'scope, 'view, 'owner> {
    parent: Option<Pin<&'scope mut StoredContext<'view, 'owner>>>,
    cached: Option<Pin<&'scope mut View<'view, 'owner>>>,
}

impl<'scope, 'view: 'scope, 'owner: 'view> CachedParent<'scope, 'view, 'owner> {
    fn initialize(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.cached.is_none() {
            ready!(self.parent.as_mut().unwrap().as_mut().poll_ready(cx));
            self.cached = Some(self.parent.take().unwrap().ready_view());
        }
        Poll::Ready(())
    }
}

fn main() {
    let text = String::from("non-static data");
    let slice = text.as_str();
    let mut parent = pin!(StoredContext::new(View::new(&slice)));
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut projected = CachedParent {
            parent: Some(parent.as_mut()),
            cached: None,
        };
        assert!(projected.initialize(&mut cx).is_ready());
        let address = &**projected.cached.as_ref().unwrap() as *const _;
        projected.cached.as_ref().unwrap().visits.set(1);
        assert!(projected.initialize(&mut cx).is_ready());
        assert_eq!(address, &**projected.cached.as_ref().unwrap() as *const _);
    }
    assert_eq!(parent.as_mut().ready_view().visits.get(), 1);
}

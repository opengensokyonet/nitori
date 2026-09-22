mod protocol;

use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use protocol::{BorrowedFamily, HostContext, StoredContext, View};

fn borrow_dyn_view<'access, 'view, 'owner: 'view>(
    context: Pin<&'access mut (dyn HostContext<'view, Family = BorrowedFamily<'owner>> + 'access)>,
    cx: &mut Context<'_>,
) -> Poll<Pin<&'access mut View<'view, 'owner>>> {
    context.poll_view(cx)
}

fn ready<T>(poll: Poll<T>) -> T {
    match poll {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("stored view must be ready"),
    }
}

fn main() {
    let owner = String::from("borrowed family and view");
    let text = owner.as_str();
    let mut context = StoredContext::new(View::new(&text));
    let mut cx = Context::from_waker(Waker::noop());

    let first_address = {
        let pinned = Pin::new(&mut context);
        let view = ready(borrow_dyn_view(pinned, &mut cx));
        view.visits.set(1);
        assert_eq!(*view.text, "borrowed family and view");
        &*view as *const _
    };
    let second_address = {
        let pinned = Pin::new(&mut context);
        let view = ready(borrow_dyn_view(pinned, &mut cx));
        assert_eq!(view.visits.get(), 1);
        view.visits.set(2);
        &*view as *const _
    };
    assert_eq!(first_address, second_address);
}

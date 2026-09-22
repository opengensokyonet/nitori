mod protocol;

use std::pin::Pin;

use protocol::View;

fn replace<'view, 'owner>(
    view: Pin<&mut View<'view, 'owner>>,
    replacement: View<'view, 'owner>,
) -> View<'view, 'owner> {
    // Moving the pinned !Unpin value out via a mutable reference is rejected.
    std::mem::replace(view.get_mut(), replacement)
}

fn main() {}

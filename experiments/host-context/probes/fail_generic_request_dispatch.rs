use std::pin::Pin;

trait Request {
    fn execute<V>(&mut self, view: Pin<&mut V>);
}

// Erasing the request instead of the view does not supply a vtable containing
// implementations for every unknown V. A concrete operation/view pair or an
// agreed capability interface is still needed.
fn dispatch(request: &mut dyn Request) {
    let _ = request;
}

fn main() {}

mod protocol;

use protocol::View;

fn shorten<'short, 'long: 'short, 'owner: 'long>(
    view: View<'long, 'owner>,
) -> View<'short, 'owner> {
    // Reborrowing a view is supported; shortening its internal lifetime is not.
    view
}

fn main() {}

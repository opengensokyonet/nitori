use std::pin::Pin;
use std::sync::MutexGuard;

fn rebuild<'access, 'view: 'access, T>(
    guard: Pin<&'access mut MutexGuard<'view, T>>,
) -> MutexGuard<'access, T> {
    // The old receiver shape requires a new owning view from a borrowed one.
    *guard
}

fn main() {}

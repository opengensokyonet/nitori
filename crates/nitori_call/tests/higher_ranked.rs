//! Elided lifetimes inside function pointer types and `Fn` sugar stay
//! higher-ranked: the macros name only the elided lifetimes of the arguments
//! themselves.
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{Direct, ReceiverAccess, ReceiverFamily, call, capability};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// Counts the bytes that `keep` keeps.
#[capability(sync)]
pub trait Count: ReceiverFamily {
    fn poll_count<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        keep: &dyn Fn(&u8) -> bool,
    ) -> Poll<usize>
    where
        Self: 'visit;
}
impl Count for Direct<&[u8]> {
    fn poll_count<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        keep: &dyn Fn(&u8) -> bool,
    ) -> Poll<usize>
    where
        Self: 'visit,
    {
        Poll::Ready(
            host.get_pin_mut()
                .get_mut()
                .iter()
                .filter(|b| keep(b))
                .count(),
        )
    }
}

#[call(sync)]
async fn count_pointer(
    io: &mut impl ReceiverAccess<Family: Count>,
    keep: fn(&u8) -> bool,
) -> usize {
    io.count(&keep).await
}

#[call(sync)]
async fn count_closure(
    io: &mut impl ReceiverAccess<Family: Count>,
    keep: impl Fn(&u8) -> bool,
) -> usize {
    io.count(&keep).await
}

#[call(sync)]
async fn count_boxed(
    io: &mut impl ReceiverAccess<Family: Count>,
    keep: Box<dyn Fn(&u8) -> bool + '_>,
) -> usize {
    io.count(&*keep).await
}

#[call(sync)]
async fn count_both<F: Count>(
    first: &mut impl ReceiverAccess<Family = F>,
    second: &mut impl ReceiverAccess<Family = F>,
    keep: fn(&u8) -> bool,
) -> usize {
    first.count(&keep).await + second.count(&keep).await
}

fn is_b(byte: &u8) -> bool {
    *byte == b'b'
}

#[test]
fn higher_ranked_arguments_keep_their_binders() {
    let mut input = b"abcb".as_slice();
    assert_eq!(input.sync_count_unpin(&is_b), 2);
    assert_eq!(input.sync_count_pointer_unpin(is_b), 2);
    assert_eq!(input.sync_count_closure_unpin(|byte: &u8| *byte != b'b'), 2);
    assert_eq!(input.sync_count_boxed_unpin(Box::new(is_b)), 2);
    let mut both = (b"ab".as_slice(), b"bb".as_slice());
    assert_eq!(both.sync_count_both_unpin(is_b), 3);
}

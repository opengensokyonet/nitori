#![feature(impl_trait_in_assoc_type)]
use bytes::{BufMut, Bytes, BytesMut};
use core::{
    convert::Infallible,
    future::poll_fn,
    num::NonZeroUsize,
    pin::{Pin, pin},
    task::{Context, Poll},
};
use nitori_call::{Receiver, ReceiverAccess, call};
use nitori_io::{
    Infinite,
    adapters::{Chunked, ReadLimit},
    prelude::*,
};
use std::collections::VecDeque;

async fn query<A: ReceiverAccess>(io: &mut A) -> <A::Family as ExactLen>::Length
where
    A::Family: ExactLen,
{
    poll_fn(|cx| io.poll_access(cx, |mut view, _| Poll::Ready(view.remaining_len()))).await
}

#[call(sync)]
async fn query_len<F: ExactLen>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> <F as ExactLen>::Length {
    query(io).await
}

#[test]
fn queries_memory_sources_without_consuming_them() {
    let mut slice = &b"abc"[..];
    assert_eq!(slice.sync_query_len_unpin(), 3);
    assert_eq!(slice, &b"abc"[..]);
    for bytes in [Bytes::new(), Bytes::from_static(b"abc")] {
        let mut host = bytes.clone();
        assert_eq!(host.sync_query_len_unpin(), bytes.len());
        assert_eq!(host, bytes);
    }
    assert_eq!(BytesMut::from(&b"x"[..]).sync_query_len_unpin(), 1);
    assert_eq!(VecDeque::from([4u8, 5]).sync_query_len_unpin(), 2);
    let mut chunked = Chunked::new(slice, NonZeroUsize::MIN);
    assert_eq!(chunked.sync_query_len_unpin(), 3);
    let _ = chunked.sync_read_chunk_unpin(NonZeroUsize::MIN).unwrap();
    assert_eq!(chunked.sync_query_len_unpin(), 2);
}

#[cfg(feature = "std")]
#[test]
fn cursor_length_stops_at_zero_past_eof() {
    use nitori_call::Direct;
    use std::io::Cursor;
    fn check<T>(bytes: T)
    where
        Cursor<T>: Receiver<Family = Direct<Cursor<T>>> + Unpin,
        Direct<Cursor<T>>: ExactLen<Length = usize>,
    {
        let mut cur = Cursor::new(bytes);
        for (pos, left) in [(0, 3), (2, 1), (3, 0), (u64::MAX, 0)] {
            cur.set_position(pos);
            assert_eq!(cur.sync_query_len_unpin(), left);
            assert_eq!(cur.position(), pos);
        }
    }
    check([0u8; 3]);
    check(&b"abc"[..]);
    check(vec![0u8; 3]);
    check(vec![0u8; 3].into_boxed_slice());
    check(Bytes::from_static(b"abc"));
    check(BytesMut::from(&b"abc"[..]));
}

#[call(sync)]
async fn bounded<F: ReadChunk + ExactLen<Length = usize>>(
    io: &mut impl ReceiverAccess<Family = F>,
    quota: u8,
) -> (u8, u8, u8, u8)
where
    <F as Read>::Error: core::fmt::Debug,
{
    let mut src = io.compose(ReadLimit(quota));
    let before = query(&mut src).await;
    src.read_array::<1>().await.unwrap();
    let after_byte = query(&mut src).await;
    while src
        .read_chunk(NonZeroUsize::new(5).unwrap())
        .await
        .unwrap()
        .is_some()
    {}
    (
        before,
        after_byte,
        query(&mut src).await,
        src.into_state().0,
    )
}

#[test]
fn length_tracks_both_read_paths_and_distinguishes_early_eof_from_budget() {
    let mut bytes = &b"abcdefgh"[..];
    assert_eq!(bytes.sync_bounded_unpin(3), (3, 2, 0, 0));
    assert_eq!(bytes, b"defgh");
    assert_eq!(bytes.sync_bounded_unpin(8), (5, 4, 0, 3));
}

struct Endless;
nitori_call::family_receiver!(impl [] for Endless);
impl ExactLen for Endless {
    type Length = Infinite;
    fn remaining_len<'v>(_: Pin<&mut Self::ReceiverView<'v>>) -> Infinite
    where
        Self: 'v,
    {
        Infinite
    }
}
impl Read for Endless {
    type Error = Infallible;
    fn poll_read<'v, O: BufMut + ?Sized>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'v,
    {
        // Deliberately short reads.
        let n = out.remaining_mut().min(2);
        out.put_bytes(0, n);
        Poll::Ready(Ok(n))
    }
}

#[call(sync)]
async fn wide<F: Read + ExactLen<Length = Infinite>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (u128, u128, u128)
where
    <F as Read>::Error: core::fmt::Debug,
{
    let mut outer = io.compose(ReadLimit(Infinite));
    let _: Infinite = query(&mut outer).await;
    let mut inner = outer.compose(ReadLimit(u128::MAX));
    let before = query(&mut inner).await;
    let mut dst = [0xff; 4];
    assert_eq!(inner.read(&mut dst.as_mut_slice()).await.unwrap(), 2);
    (before, query(&mut inner).await, inner.into_state().0)
}
#[test]
fn infinite_source_and_wide_quota_account_for_actual_short_reads() {
    assert_eq!(
        Endless.sync_wide_unpin(),
        (u128::MAX, u128::MAX - 2, u128::MAX - 2)
    );
    assert_eq!(
        Chunked::new(Endless, NonZeroUsize::MIN).sync_query_len_unpin(),
        Infinite
    );
}

#[test]
fn minimum_keeps_the_finite_right_type_and_accepts_owned_operands() {
    use nitori_io::Min;
    let small: u8 = Min::min(u128::MAX, 7u8);
    let wide: u128 = Min::min(3u8, u128::MAX);
    let at_native: usize = Min::min(u128::MAX, usize::MAX);
    let still_small: u8 = Min::min(7u8, Infinite);
    let both: Infinite = Min::min(Infinite, Infinite);
    assert_eq!(
        (small, wide, at_native, still_small, both),
        (7, 3, usize::MAX, 7, Infinite)
    );
    struct Owned(String);
    impl Min<usize> for Owned {
        type Output = usize;
        fn min(self, rhs: usize) -> usize {
            core::cmp::min(self.0.len(), rhs)
        }
    }
    assert_eq!(Min::min(Owned(String::from("abc")), 4usize), 3);
}

struct Unknown {
    polls: usize,
}
nitori_call::family_receiver!(impl [] for Unknown);
impl Read for Unknown {
    type Error = ();
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<usize, ()>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        this.polls += 1;
        match this.polls {
            1 => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            2 => Poll::Ready(Err(())),
            _ => {
                out.put_u8(7);
                Poll::Ready(Ok(1))
            }
        }
    }
}
#[test]
fn pending_and_error_leave_quota_untouched_even_for_an_unknown_source() {
    use core::task::Waker;
    use nitori_call::Compose;
    use nitori_io::adapters::Limited;
    let mut host = Unknown { polls: 0 };
    let mut quota = ReadLimit(255u8);
    let mut output = Vec::new();
    for (want, left) in [
        (Poll::Pending, 255),
        (Poll::Ready(Err(())), 255),
        (Poll::Ready(Ok(1)), 254),
    ] {
        let mut parent = pin!(Pin::new(&mut host).view());
        let view =
            <ReadLimit<u8> as Compose<Unknown>>::compose(Pin::new(&mut quota), parent.as_mut());
        let got = <Limited<Unknown, ReadLimit<u8>> as Read>::poll_read(
            pin!(view),
            &mut Context::from_waker(Waker::noop()),
            &mut output,
        );
        assert_eq!(got, want);
        assert_eq!(quota.0, left);
    }
    assert_eq!(output, [7]);
}

#[test]
fn unsigned_minimum_matches_mathematical_minimum_at_each_width() {
    macro_rules! pairs {
        ($left:ty; $($right:ty),+) => {$({
            for left in [0 as $left, 1, <$left>::MAX / 2, <$left>::MAX] {
                for right in [0 as $right, 1, <$right>::MAX / 2, <$right>::MAX] {
                    let got: $right = nitori_io::Min::min(left,right);
                    assert_eq!(got as u128, core::cmp::min(left as u128,right as u128));
                }
            }
        })+};
    }
    macro_rules! widths {
        ($($width:ty),+) => {$(pairs!($width; u8,u16,u32,u64,u128,usize);)+};
    }
    widths!(u8, u16, u32, u64, u128, usize);
}

#[call(sync)]
async fn nested<F: Read + ExactLen<Length = usize>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (usize, u8, u64, u8, usize)
where
    <F as Read>::Error: core::fmt::Debug,
{
    let mut unbounded = io.compose(ReadLimit(Infinite));
    let initial = query(&mut unbounded).await;
    let mut small = unbounded.compose(ReadLimit(3u8));
    let small_len = query(&mut small).await;
    let mut large = small.compose(ReadLimit(1u64 << 40));
    large.read_array::<2>().await.unwrap();
    let large_len = query(&mut large).await;
    assert_eq!(large.into_state().0, (1u64 << 40) - 2);
    let small_left = small.into_state().0;
    (
        initial,
        small_len,
        large_len,
        small_left,
        query(&mut unbounded).await,
    )
}

#[test]
fn nested_quotas_keep_their_own_width_and_share_a_single_cursor() {
    let mut bytes = &b"abcd"[..];
    assert_eq!(bytes.sync_nested_unpin(), (4, 3, 1, 1, 2));
    assert_eq!(bytes, b"cd");
}

#[test]
fn zero_sized_infinite_quota_is_unchanged_by_subassign() {
    let mut quota = Infinite;
    quota -= usize::MAX;
    assert_eq!(quota, Infinite);
    assert_eq!(core::mem::size_of::<ReadLimit<Infinite>>(), 0);
    assert_eq!(core::mem::size_of::<ReadLimit<u8>>(), 1);
}

#[test]
fn budget_views_display_the_remaining_quota() {
    // Existing downstream code can format a view with an unconstrained marker.
    fn describe<F: nitori_call::ReceiverFamily, L>(
        view: &nitori_io::adapters::LimitedView<'_, F, L>,
    ) -> String {
        format!("{view:?}")
    }
    use nitori_call::{Compose, Direct};
    use nitori_io::adapters::WriteLimit;
    let mut storage = Vec::<u8>::new();
    let mut write_limit = WriteLimit(3);
    let mut view = pin!(Pin::new(&mut storage).view());
    let limited = <WriteLimit as Compose<Direct<Vec<u8>>>>::compose(
        Pin::new(&mut write_limit),
        view.as_mut(),
    );
    assert_eq!(describe(&limited), "LimitedView { remaining: 3, .. }");

    let mut source = &b"abc"[..];
    let mut read_limit = ReadLimit(7u8);
    let mut view = pin!(Pin::new(&mut source).view());
    let limited = <ReadLimit<u8> as Compose<Direct<&[u8]>>>::compose(
        Pin::new(&mut read_limit),
        view.as_mut(),
    );
    assert_eq!(describe(&limited), "LimitedView { remaining: 7, .. }");
}

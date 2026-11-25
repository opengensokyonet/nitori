//! A write limit checks the count its parent reports, as a read limit does.
use bytes::Buf;
use core::{
    convert::Infallible,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use nitori_call::{Compose, Receiver};
use nitori_io::{
    Write,
    adapters::{Limited, WriteLimit},
};

/// Reports more than it was offered, against the contract of `Write`.
struct Overcounting;
nitori_call::family_receiver!(impl [] for Overcounting);
impl Write for Overcounting {
    type Error = Infallible;
    type Report = ();
    fn poll_write<'v, I: Buf + ?Sized>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<Option<nitori_io::WriteSegment<Self::Report>>, !>>
    where
        Self: 'v,
    {
        Poll::Ready(Ok(written(input.remaining() + 10)))
    }
}

#[test]
#[should_panic = "write exceeded the offered input"]
fn a_write_counted_beyond_the_limit_panics() {
    let mut host = Overcounting;
    let mut limit = WriteLimit(3);
    let mut limited = pin!(<WriteLimit as Compose<Overcounting>>::compose(
        Pin::new(&mut limit),
        Pin::new(&mut host).view(),
    ));
    let _ = <Limited<Overcounting, WriteLimit> as Write>::poll_write(
        limited.as_mut(),
        &mut Context::from_waker(Waker::noop()),
        &mut &b"abcdef"[..],
    );
}

fn written(count: usize) -> Option<nitori_io::WriteSegment<()>> {
    core::num::NonZeroUsize::new(count).map(|count| nitori_io::WriteSegment { count, report: () })
}

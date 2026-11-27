//! A write limit checks the count its parent reports, as a read limit does.
use bytes::{Buf, BufMut};
use core::{
    convert::Infallible,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use nitori_call::Receiver;
use nitori_io::{
    Write,
    adapters::{WriteLimit, WriteLimited},
};

/// Reports more than it was offered, against the contract of `Write`.
struct Overcounting;
nitori_call::family_receiver!(impl [] for Overcounting);
impl Write<()> for Overcounting {
    type Error = Infallible;
    type Report = ();
    fn poll_write<'v, I: Buf + ?Sized>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut I,
        _: &(),
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
    let mut limit = 3;
    let mut limited = pin!(WriteLimit::borrowing(
        Pin::new(&mut host).view(),
        &mut limit
    ));
    let _ = <WriteLimited<Overcounting> as Write<()>>::poll_write(
        limited.as_mut(),
        &mut Context::from_waker(Waker::noop()),
        &mut &b"abcdef"[..],
        &(),
    );
}

fn written(count: usize) -> Option<nitori_io::WriteSegment<()>> {
    core::num::NonZeroUsize::new(count).map(|count| nitori_io::WriteSegment { count, report: () })
}

#[test]
fn a_receiver_and_its_view_share_the_same_write_budget() {
    use nitori_call::Direct;
    use nitori_io::{Filled, LentBuffer, LentOutcome, WriteLent, prelude::*};

    let mut remaining = 3;
    let mut sink = WriteLimit::borrowing(Vec::new(), &mut remaining);
    {
        let mut view = pin!(Pin::new(&mut sink).view());
        let mut cx = Context::from_waker(Waker::noop());
        let result = <WriteLimited<Direct<Vec<u8>>> as WriteLent<()>>::poll_write_lent(
            view.as_mut(),
            &mut cx,
            core::num::NonZeroUsize::new(4).unwrap(),
            |buffer: &mut LentBuffer<_>| {
                buffer.buffer().put_slice(b"a");
                Ok::<_, !>(Filled {
                    value: (),
                    options: (),
                })
            },
        );
        assert!(matches!(
            result,
            Poll::Ready(Ok(Some(LentOutcome::Kept(_))))
        ));
    }

    assert!(sink.sync_write_all_unpin(b"bc".as_slice(), ()).is_ok());
    let sink = sink.into_parts().io;
    assert_eq!(sink, b"abc");
    assert_eq!(remaining, 0);
}

#[test]
fn nested_write_wrappers_work_with_a_generic_send_receiver() {
    use core::future::Future;
    use nitori_io::{
        adapters::{FixedOptions, MappedOptions},
        prelude::*,
    };

    fn require_send<F: Future + Send>(future: F) -> F {
        future
    }

    fn write<R>(sink: &mut R, bytes: &[u8]) -> impl Future<Output = ()> + Send
    where
        R: Receiver<Family: Write<(), Error = !, Report = ()>> + Send + Unpin,
    {
        require_send(async move {
            let limited = WriteLimit::new(sink, 2);
            let mut mapped = MappedOptions::new(limited, FixedOptions(()));
            let result = mapped.write_all_unpin(bytes, false).await;
            assert_eq!(result.unwrap_err().completed(), 2);
            assert_eq!(*mapped.get_ref().budget(), 0);
        })
    }

    let payload = *b"abc";
    let mut sink = Vec::new();
    assert_eq!(
        pin!(write(&mut sink, &payload)).poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(())
    );
    assert_eq!(sink, b"ab");
}

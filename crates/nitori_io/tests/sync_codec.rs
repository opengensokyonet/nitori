#![feature(impl_trait_in_assoc_type, yield_expr)]
use futures_core::Stream;
use nitori_call::{CallExecutor, ReceiverAccess, call};
use nitori_io::PollReadExt as _;
use nitori_io::{
    Read, Write,
    calls::{ReadArrayError, ReadArrayExt as _, WriteAllError, WriteAllExt as _},
};
use std::{
    future::Future,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

#[call(sync)]
async fn header<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
) -> Result<[u8; 3], ReadArrayError<<A::Family as Read>::Error>> {
    let [tag] = io.read_array::<1>().await?;
    let rest = io.read_array::<2>().await?;
    Ok([tag, rest[0], rest[1]])
}
#[call(sync)]
async fn encode<A: ReceiverAccess<Family: Write>>(
    io: &mut A,
    input: &mut &[u8],
) -> Result<(), WriteAllError<<A::Family as Write>::Error>> {
    io.write_all(input).await
}
#[call(sync, yields = [u8; 3])]
async fn headers<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
    count: usize,
) -> Result<(), ReadArrayError<<A::Family as Read>::Error>> {
    for _ in 0..count {
        yield io.header().await?;
    }
    Ok(())
}
#[test]
fn memory_codec_reports_natural_incomplete_and_preserves_cursor() {
    let mut source = &b"abcde"[..];
    assert_eq!(source.sync_header_unpin().unwrap(), *b"abc");
    let error = source.sync_header_unpin().unwrap_err();
    assert!(matches!(error, ReadArrayError::Incomplete { .. }));
    assert_eq!(error.completed(), 1);
    assert!(source.is_empty());
    let mut storage = [0; 2];
    let mut input = &b"abcd"[..];
    let result = storage.as_mut_slice().sync_encode_unpin(&mut input);
    assert_eq!(input, b"cd");
    assert!(matches!(result, Err(WriteAllError::WriteZero { .. })));
    assert_eq!(storage, *b"ab");
}
#[test]
fn yielded_frames_stop_consumption_until_resumed() {
    let mut source = &b"abcdef"[..];
    {
        let mut events = pin!(source.headers_unpin(2));
        assert!(matches!(
            events.as_mut().poll_next(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Some(ControlFlow::Continue(frame))) if frame == *b"abc"
        ));
    }
    assert_eq!(source, b"def");
}
struct Delayed<'a> {
    source: &'a [u8],
    waiting: bool,
}
impl Read for Delayed<'_> {
    type Error = std::convert::Infallible;
    fn poll_read<'visit, Output: bytes::BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let mut this = host.get_pin_mut();
        if !this.waiting {
            this.waiting = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.waiting = false;
        Pin::new(&mut this.source).poll_read(cx, destination)
    }
}
#[test]
fn same_codec_can_resume_on_async_host() {
    let mut host = Delayed {
        source: b"abc",
        waiting: false,
    };
    let mut call = pin!(CallExecutor::execute(Pin::new(&mut host), Header::new()));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(call.as_mut().poll(&mut cx).is_pending());
    assert!(call.as_mut().poll(&mut cx).is_pending());
    assert!(matches!(call.as_mut().poll(&mut cx), Poll::Ready(Ok(value)) if value == *b"abc"));
}

nitori_call::family_receiver!(impl ['data] for Delayed<'data>);

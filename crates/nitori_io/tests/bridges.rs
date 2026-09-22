#![cfg(any(feature = "tokio", feature = "futures"))]
use bytes::{Buf, BufMut, Bytes};
use nitori_call::BoundCall;
use nitori_io::{PollReadExt as _, PollWriteExt as _};
use nitori_io::{Read, Write, adapters::Chunked, calls};
use std::{
    cell::Cell,
    future::Future,
    io,
    marker::PhantomPinned,
    num::NonZeroUsize,
    pin::{Pin, pin},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

#[derive(Clone, Copy)]
enum Action {
    Pending,
    Error,
    Ready,
}
pin_project_lite::pin_project! {
    struct Receiver {
        action: Rc<Cell<Action>>,
        source: Bytes,
        written: Vec<u8>,
        #[pin]
        marker: PhantomPinned,
    }
}
impl Receiver {
    fn new(action: Rc<Cell<Action>>) -> Self {
        Self {
            action,
            source: Bytes::from_static(b"abcdef"),
            written: Vec::new(),
            marker: PhantomPinned,
        }
    }
    fn read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        // Scratch writes on failed polls must not leak into the destination.
        output.fill(0xff);
        match this.action.get() {
            Action::Pending => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Action::Error => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "read detail",
            ))),
            Action::Ready => {
                let count = output.len().min(2).min(this.source.len());
                output[..count].copy_from_slice(&this.source[..count]);
                this.source.advance(count);
                Poll::Ready(Ok(count))
            }
        }
    }
    fn write(self: Pin<&mut Self>, cx: &mut Context<'_>, input: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.project();
        match this.action.get() {
            Action::Pending => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Action::Error => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "write detail",
            ))),
            Action::Ready => {
                let count = input.len().min(2);
                this.written.extend_from_slice(&input[..count]);
                Poll::Ready(Ok(count))
            }
        }
    }
}
#[cfg(feature = "tokio")]
impl tokio::io::AsyncRead for Receiver {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let count = std::task::ready!(self.read(cx, output.initialize_unfilled()))?;
        output.advance(count);
        Poll::Ready(Ok(()))
    }
}
#[cfg(feature = "tokio")]
impl tokio::io::AsyncWrite for Receiver {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.write(cx, input)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        panic!("implicit flush")
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        panic!("implicit shutdown")
    }
}
#[cfg(feature = "futures")]
impl futures_io::AsyncRead for Receiver {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.read(cx, output)
    }
}
#[cfg(feature = "futures")]
impl futures_io::AsyncWrite for Receiver {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.write(cx, input)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        panic!("implicit flush")
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        panic!("implicit close")
    }
}
#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
fn ready<T>(value: Poll<io::Result<T>>) -> T {
    match value {
        Poll::Ready(result) => result.unwrap(),
        Poll::Pending => panic!("unexpected Pending"),
    }
}
fn check<T: nitori_call::Receiver>(host: T, action: Rc<Cell<Action>>)
where
    T::Family: Read<Error = io::Error> + Write<Error = io::Error>,
{
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut host = pin!(Chunked::new(host, NonZeroUsize::new(3).unwrap()));
    let mut output = vec![b'x'];
    let mut input = Bytes::from_static(b"12345");
    assert!(host.as_mut().poll_read(&mut cx, &mut output).is_pending());
    assert!(host.as_mut().poll_write(&mut cx, &mut input).is_pending());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 2);
    assert_eq!(output, b"x");
    assert_eq!(input, b"12345"[..]);
    action.set(Action::Error);
    let Poll::Ready(Err(error)) = host.as_mut().poll_read(&mut cx, &mut output) else {
        panic!()
    };
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(error.to_string(), "read detail");
    let Poll::Ready(Err(error)) = host.as_mut().poll_write(&mut cx, &mut input) else {
        panic!()
    };
    assert_eq!(error.to_string(), "write detail");
    assert_eq!(output, b"x");
    assert_eq!(input, b"12345"[..]);
    assert!(matches!(
        host.as_mut()
            .poll_read(&mut cx, &mut [0u8; 0].as_mut_slice()),
        Poll::Ready(Err(_))
    ));
    assert!(matches!(
        host.as_mut().poll_write(&mut cx, &mut &b""[..]),
        Poll::Ready(Err(_))
    ));
    action.set(Action::Ready);
    assert_eq!(ready(host.as_mut().poll_read(&mut cx, &mut output)), 2);
    assert_eq!(output, b"xab");
    assert_eq!(ready(host.as_mut().poll_write(&mut cx, &mut input)), 2);
    assert_eq!(input, b"345"[..]);
    // Cancellation while waiting for a chunk must consume nothing.
    action.set(Action::Pending);
    {
        let mut call = pin!(BoundCall::new(
            host.as_mut(),
            calls::ReadChunk::new(NonZeroUsize::new(1).unwrap())
        ));
        assert!(call.as_mut().poll(&mut cx).is_pending());
    }
    // A larger pending request replaces the smaller scratch allocation.
    assert!(
        host.as_mut()
            .poll_read_chunk(&mut cx, NonZeroUsize::new(99).unwrap())
            .is_pending()
    );
    action.set(Action::Ready);
    let chunk = ready(
        host.as_mut()
            .poll_read_chunk(&mut cx, NonZeroUsize::new(1).unwrap()),
    )
    .unwrap();
    assert_eq!(chunk, b"c"[..]);
    let mut left = [0; 1];
    let mut right = [0; 2];
    let mut output = left.as_mut_slice().chain_mut(right.as_mut_slice());
    assert_eq!(
        ready(
            host.as_mut()
                .poll_read(&mut cx, &mut output as &mut dyn BufMut)
        ),
        2
    );
    assert_eq!(left, *b"d");
    assert_eq!(right, [b'e', 0]);
    let chunk = ready(
        host.as_mut()
            .poll_read_chunk(&mut cx, NonZeroUsize::new(99).unwrap()),
    )
    .unwrap();
    assert_eq!(chunk, b"f"[..]);
    assert!(
        ready(
            host.as_mut()
                .poll_read_chunk(&mut cx, NonZeroUsize::new(1).unwrap())
        )
        .is_none()
    );
}
#[test]
#[cfg(feature = "tokio")]
fn tokio_preserves_poll_contract_for_pinned_local_hosts() {
    let action = Rc::new(Cell::new(Action::Pending));
    check(
        nitori_io::bridge::Tokio::new(Receiver::new(action.clone())),
        action,
    );
}
#[test]
#[cfg(feature = "futures")]
fn futures_preserves_poll_contract_for_pinned_local_hosts() {
    let action = Rc::new(Cell::new(Action::Pending));
    check(
        nitori_io::bridge::Futures::new(Receiver::new(action.clone())),
        action,
    );
}

#[test]
fn pinned_pointer_forwards_to_non_unpin_host() {
    let action = Rc::new(Cell::new(Action::Pending));
    #[cfg(feature = "tokio")]
    let host = nitori_io::bridge::Tokio::new(Receiver::new(action.clone()));
    #[cfg(all(feature = "futures", not(feature = "tokio")))]
    let host = nitori_io::bridge::Futures::new(Receiver::new(action.clone()));
    check(Box::pin(host), action);
}

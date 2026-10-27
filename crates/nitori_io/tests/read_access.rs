#![cfg(any(feature = "tokio", feature = "futures"))]
#![feature(impl_trait_in_assoc_type)]
use bytes::BufMut;
use nitori_call::{Call, CallPoll, Lend, Receiver, ReceiverAccess, call};
use nitori_io::{Read, bridge::ReadAccess};
use std::{
    io,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

/// Gives one byte per read and waits before each.
struct Trickle {
    bytes: &'static [u8],
    waited: bool,
}
nitori_call::family_receiver!(impl [] for Trickle);
impl Read for Trickle {
    type Error = io::Error;
    fn poll_read<'visit, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut B,
    ) -> Poll<io::Result<usize>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut().get_mut();
        if !this.waited {
            this.waited = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.waited = false;
        let Some((&first, rest)) = this.bytes.split_first() else {
            return Poll::Ready(Ok(0));
        };
        destination.put_u8(first);
        this.bytes = rest;
        Poll::Ready(Ok(1))
    }
}

/// Poll a call with a view of `host` until it completes.
fn run<Op>(operation: Op, host: &mut Trickle) -> <Op as nitori_call::Operation<Trickle>>::Output
where
    Op: nitori_call::Operation<Trickle>
        + for<'s> nitori_call::Start<'s, nitori_call::ReceiverSlot<'s, Trickle>>,
{
    let mut call = pin!(Call::<Trickle, _>::new(operation));
    for _ in 0..100 {
        let view = pin!(Pin::new(&mut *host).view());
        if let CallPoll::Ready(output) = call.as_mut().poll_return_with(
            Some(Lend::Fixed(view)),
            &mut Context::from_waker(Waker::noop()),
        ) {
            return output;
        }
    }
    panic!("the call did not complete")
}

#[cfg(feature = "tokio")]
#[call]
async fn tokio_header<A: ReceiverAccess<Family: Read<Error = io::Error>>>(
    io: &mut A,
) -> io::Result<(u16, [u8; 2])> {
    use tokio::io::AsyncReadExt;
    let mut reader = ReadAccess::new(io);
    let length = reader.read_u16().await?;
    let mut tag = [0; 2];
    reader.read_exact(&mut tag).await?;
    Ok((length, tag))
}

#[test]
#[cfg(feature = "tokio")]
fn a_tokio_codec_runs_inside_an_operation_across_pending() {
    let mut host = Trickle {
        bytes: b"\x01\x02abc",
        waited: false,
    };
    let (length, tag) = run(TokioHeader::new(), &mut host).unwrap();
    assert_eq!((length, &tag), (0x0102, b"ab"));
    // The reader buffers nothing: the next byte is still in the source.
    assert_eq!(host.bytes, b"c");
}

#[test]
#[cfg(feature = "tokio")]
fn a_tokio_codec_reports_the_end_of_the_source() {
    let mut host = Trickle {
        bytes: b"\x01",
        waited: false,
    };
    let error = run(TokioHeader::new(), &mut host).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
}

#[cfg(feature = "futures")]
#[call]
async fn futures_read<A: ReceiverAccess<Family: Read<Error = io::Error>>>(
    io: &mut A,
) -> io::Result<Vec<u8>> {
    let mut reader = ReadAccess::new(io);
    let mut buffer = [0; 4];
    let count = std::future::poll_fn(|cx| {
        futures_io::AsyncRead::poll_read(Pin::new(&mut reader), cx, &mut buffer)
    })
    .await?;
    Ok(buffer[..count].to_vec())
}

#[test]
#[cfg(feature = "futures")]
fn a_futures_reader_reads_through_the_access() {
    let mut host = Trickle {
        bytes: b"xy",
        waited: false,
    };
    assert_eq!(run(FuturesRead::new(), &mut host).unwrap(), b"x");
    assert_eq!(host.bytes, b"y");
}

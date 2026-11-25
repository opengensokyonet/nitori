#![cfg(any(feature = "tokio", feature = "futures"))]
#![feature(impl_trait_in_assoc_type)]
use bytes::BufMut;
use nitori_call::{Call, CallPoll, Lend, Receiver, ReceiverAccess, call};
use nitori_io::{Combine, Read, bridge::ReadAccess};
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
    type Metadata = ();
    fn poll_read<'visit, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut B,
    ) -> Poll<io::Result<Option<nitori_io::ReadSegment<()>>>>
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
            return Poll::Ready(Ok(None));
        };
        destination.put_u8(first);
        this.bytes = rest;
        Poll::Ready(Ok(segment(1)))
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
async fn tokio_header<A: ReceiverAccess<Family: Read<Error = io::Error, Metadata = ()>>>(
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
async fn futures_read<A: ReceiverAccess<Family: Read<Error = io::Error, Metadata = ()>>>(
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

fn segment(count: usize) -> Option<nitori_io::ReadSegment<()>> {
    core::num::NonZeroUsize::new(count).map(|count| nitori_io::ReadSegment {
        count,
        metadata: (),
    })
}

/// A mark that merges to "some segment was marked", so that no segments merge
/// to "none".
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Mark(bool);
impl Combine for Mark {
    fn combine(self, next: Self) -> Self {
        Mark(self.0 || next.0)
    }
}

/// Gives one byte per read, each with its own mark.
struct Marked {
    bytes: &'static [(u8, bool)],
}
nitori_call::family_receiver!(impl [] for Marked);
impl Read for Marked {
    type Error = io::Error;
    type Metadata = Mark;
    fn poll_read<'visit, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        destination: &mut B,
    ) -> Poll<io::Result<Option<nitori_io::ReadSegment<Mark>>>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut().get_mut();
        let Some((&(byte, mark), rest)) = this.bytes.split_first() else {
            return Poll::Ready(Ok(None));
        };
        destination.put_u8(byte);
        this.bytes = rest;
        Poll::Ready(Ok(Some(nitori_io::ReadSegment {
            count: core::num::NonZeroUsize::MIN,
            metadata: Mark(mark),
        })))
    }
}

fn run_marked<Op>(
    operation: Op,
    host: &mut Marked,
) -> <Op as nitori_call::Operation<Marked>>::Output
where
    Op: nitori_call::Operation<Marked>
        + for<'s> nitori_call::Start<'s, nitori_call::ReceiverSlot<'s, Marked>>,
{
    let mut call = pin!(Call::<Marked, _>::new(operation));
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

/// The metadata seen after each step: merged so far, then taken, then after
/// taking.
type Seen = (Mark, Mark, Mark);

#[cfg(feature = "tokio")]
#[call]
async fn tokio_marked<A: ReceiverAccess<Family: Read<Error = io::Error, Metadata = Mark>>>(
    io: &mut A,
) -> io::Result<(Seen, Seen)> {
    use tokio::io::AsyncReadExt;
    let mut reader = ReadAccess::new(io);
    let mut two = [0; 2];
    reader.read_exact(&mut two).await?;
    let first = (
        *reader.metadata(),
        reader.take_metadata(),
        *reader.metadata(),
    );
    reader.read_exact(&mut two).await?;
    let second = (
        *reader.metadata(),
        reader.take_metadata(),
        *reader.metadata(),
    );
    Ok((first, second))
}

#[test]
#[cfg(feature = "tokio")]
fn a_tokio_reader_merges_the_metadata_of_what_it_read_and_resets_on_take() {
    let mut host = Marked {
        bytes: &[(1, false), (2, true), (3, false), (4, false)],
    };
    let (first, second) = run_marked(TokioMarked::new(), &mut host).unwrap();
    // The marked byte is merged in; taking hands it over and resets.
    assert_eq!(first, (Mark(true), Mark(true), Mark(false)));
    // Nothing marked in the second message.
    assert_eq!(second, (Mark(false), Mark(false), Mark(false)));
}

#[cfg(feature = "futures")]
#[call]
async fn futures_marked<A: ReceiverAccess<Family: Read<Error = io::Error, Metadata = Mark>>>(
    io: &mut A,
) -> io::Result<(Seen, Seen)> {
    let mut reader = ReadAccess::new(io);
    let mut buffer = [0; 1];
    let mut seen = Vec::new();
    for _ in 0..2 {
        for _ in 0..2 {
            let count = std::future::poll_fn(|cx| {
                futures_io::AsyncRead::poll_read(Pin::new(&mut reader), cx, &mut buffer)
            })
            .await?;
            assert_eq!(count, 1);
        }
        seen.push((
            *reader.metadata(),
            reader.take_metadata(),
            *reader.metadata(),
        ));
    }
    Ok((seen[0], seen[1]))
}

#[test]
#[cfg(feature = "futures")]
fn a_futures_reader_merges_the_metadata_of_what_it_read_and_resets_on_take() {
    let mut host = Marked {
        bytes: &[(1, false), (2, true), (3, false), (4, false)],
    };
    let (first, second) = run_marked(FuturesMarked::new(), &mut host).unwrap();
    assert_eq!(first, (Mark(true), Mark(true), Mark(false)));
    assert_eq!(second, (Mark(false), Mark(false), Mark(false)));
}

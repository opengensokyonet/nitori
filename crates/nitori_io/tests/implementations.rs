use bytes::{Buf, BufMut, Bytes, BytesMut};
use nitori_io::{Read, ReadChunk, Write, adapters::Chunked, bridge::Std};
use std::{
    collections::VecDeque,
    convert::Infallible,
    io::{self, Cursor},
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, Waker},
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}
fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}
fn ready<T, E: std::fmt::Debug>(poll: Poll<Result<T, E>>) -> T {
    match poll {
        Poll::Ready(result) => result.unwrap(),
        Poll::Pending => panic!("unexpected Pending"),
    }
}
fn check_reader<T: Read<Error = Infallible> + Unpin>(mut source: T) {
    let mut first = [0; 1];
    let mut second = [0; 2];
    let mut output = first.as_mut_slice().chain_mut(second.as_mut_slice());
    let output: &mut dyn BufMut = &mut output;
    assert_eq!(ready(Pin::new(&mut source).poll_read(&mut cx(), output)), 3);
    assert_eq!(first, *b"a");
    assert_eq!(second, *b"bc");
    let mut rest = Vec::new();
    assert_eq!(
        ready(Pin::new(&mut source).poll_read(&mut cx(), &mut rest)),
        2
    );
    assert_eq!(rest, b"de");
    assert_eq!(
        ready(Pin::new(&mut source).poll_read(&mut cx(), &mut rest)),
        0
    );
}
#[test]
fn memory_readers_and_forwarding_share_cursors() {
    check_reader(&b"abcde"[..]);
    check_reader(Bytes::from_static(b"abcde"));
    check_reader(BytesMut::from(&b"abcde"[..]));
    let mut queue = VecDeque::with_capacity(5);
    queue.extend(b"xxabc");
    queue.drain(..2);
    queue.extend(b"de");
    assert!(!queue.as_slices().1.is_empty());
    check_reader(queue);
    check_reader(Cursor::new(b"abcde"));
    check_reader(Box::new(Bytes::from_static(b"abcde")));
    let mut borrowed = Bytes::from_static(b"abcde");
    check_reader(&mut borrowed);
    assert!(borrowed.is_empty());
    check_reader(Box::pin(Bytes::from_static(b"abcde")));
    let mut cursor = Cursor::new(b"abc");
    cursor.set_position(u64::MAX);
    assert_eq!(
        ready(Pin::new(&mut cursor).poll_read(&mut cx(), &mut Vec::new())),
        0
    );
    assert_eq!(cursor.position(), u64::MAX);
}
fn check_chunks<T: ReadChunk<Error = Infallible> + Unpin>(mut source: T) {
    let mut first = [0; 1];
    ready(Pin::new(&mut source).poll_read(&mut cx(), &mut first.as_mut_slice()));
    assert_eq!(first, *b"a");
    let chunk = ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(2))).unwrap();
    assert_eq!(chunk.chunk(), b"bc");
    let chunk = ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(99))).unwrap();
    assert_eq!(chunk.chunk(), b"de");
    assert!(ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(1))).is_none());
}
#[test]
fn native_chunks_are_bounded_and_zero_copy() {
    let bytes = Bytes::from_static(b"abcde");
    let address = bytes.as_ptr();
    let mut source = bytes.clone();
    let chunk = ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(2))).unwrap();
    assert_eq!(chunk.as_ptr(), address);
    check_chunks(bytes);
    check_chunks(BytesMut::from(&b"abcde"[..]));
    check_chunks(&b"abcde"[..]);
    check_chunks(Box::pin(Bytes::from_static(b"abcde")));
}
fn write<T: Write + Unpin>(sink: &mut T) -> usize
where
    T::Error: std::fmt::Debug,
{
    let mut input = (&b"ab"[..]).chain(&b"cde"[..]);
    let input: &mut dyn Buf = &mut input;
    let count = ready(Pin::new(sink).poll_write(&mut cx(), input));
    assert_eq!(input.remaining(), 5 - count);
    count
}
#[test]
fn memory_writers_advance_segmented_input() {
    let mut vector = b"prefix".to_vec();
    assert_eq!(write(&mut vector), 5);
    assert_eq!(vector, b"prefixabcde");
    let mut bytes = BytesMut::new();
    assert_eq!(write(&mut bytes), 5);
    assert_eq!(bytes, b"abcde"[..]);
    let mut queue = VecDeque::new();
    assert_eq!(write(&mut queue), 5);
    assert_eq!(Vec::from(queue), b"abcde");
    let mut fixed = [0; 3];
    let mut sink = fixed.as_mut_slice();
    assert_eq!(write(&mut sink), 3);
    assert_eq!(write(&mut sink), 0);
    assert_eq!(fixed, *b"abc");
    let mut cursor = Cursor::new(vec![b'x'; 4]);
    cursor.set_position(1);
    assert_eq!(write(&mut cursor), 2); // first contiguous input segment
    assert_eq!(cursor.position(), 3);
    assert_eq!(cursor.into_inner(), b"xabx");
    let mut boxed = Box::new(Vec::new());
    assert_eq!(write(&mut boxed), 5);
    let mut pinned = Box::pin(Vec::new());
    assert_eq!(write(&mut pinned), 5);
}
#[test]
fn std_bridge_and_copying_chunks() {
    let mut source = Chunked::new(Std::new(Cursor::new(b"abcdef")), nz(2));
    assert_eq!(source.chunk_capacity(), nz(2));
    assert_eq!(
        ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(99))).unwrap(),
        b"ab"[..]
    );
    let mut one = [0; 1];
    ready(Pin::new(&mut source).poll_read(&mut cx(), &mut one.as_mut_slice()));
    assert_eq!(one, *b"c");
    assert_eq!(
        ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(1))).unwrap(),
        b"d"[..]
    );
    assert_eq!(source.into_inner().into_inner().position(), 4);
    let mut sink = Chunked::new(Std::new(Vec::new()), nz(1));
    assert_eq!(write(&mut sink), 2);
    assert_eq!(sink.into_inner().into_inner(), b"ab");
}
struct ErrorHost(io::ErrorKind);
impl io::Read for ErrorHost {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        buffer.fill(0xff);
        Err(io::Error::new(self.0, "source detail"))
    }
}
impl io::Write for ErrorHost {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(self.0, "sink detail"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn std_errors_preserve_buffers_and_empty_requests_reach_host() {
    for kind in [
        io::ErrorKind::WouldBlock,
        io::ErrorKind::Interrupted,
        io::ErrorKind::Other,
    ] {
        let mut host = Std::new(ErrorHost(kind));
        let mut output = vec![1];
        let Poll::Ready(Err(error)) = Pin::new(&mut host).poll_read(&mut cx(), &mut output) else {
            panic!()
        };
        assert_eq!(error.kind(), kind);
        assert_eq!(error.to_string(), "source detail");
        assert_eq!(output, [1]);
        assert!(matches!(
            Pin::new(&mut host).poll_read(&mut cx(), &mut [0u8; 0].as_mut_slice()),
            Poll::Ready(Err(_))
        ));
        for mut input in [&b"abc"[..], &b""[..]] {
            let before = input;
            assert!(matches!(
                Pin::new(&mut host).poll_write(&mut cx(), &mut input),
                Poll::Ready(Err(_))
            ));
            assert_eq!(input, before);
        }
    }
}

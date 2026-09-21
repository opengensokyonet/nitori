#![feature(
    coroutine_trait,
    coroutines,
    stmt_expr_attributes,
    type_alias_impl_trait
)]
use bytes::{Buf, BufMut, Bytes, BytesMut};
use nitori_call::{BoundCall, call};
use nitori_io::{PollReadExt as _, ReceiverReadExt as _, ReceiverWriteExt as _};
use nitori_io::{
    Read as ReadHost, ReadChunk as ChunkHost, Write as WriteHost,
    calls::*,
    error::{Incomplete, InsufficientCapacity, WriteZero},
    helpers::*,
};
use std::{
    cell::Cell,
    future::Future,
    io,
    num::NonZeroUsize,
    ops::CoroutineState,
    pin::{Pin, pin},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}
#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
fn run<F: Future>(future: F) -> F::Output {
    let wake = Arc::new(Wakes::default());
    let waker = Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    for _ in 0..1000 {
        let before = wake.0.load(Ordering::Relaxed);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => assert!(
                wake.0.load(Ordering::Relaxed) > before,
                "Pending without waking"
            ),
        }
    }
    panic!("operation did not finish")
}

struct Source {
    bytes: Bytes,
    quantum: usize,
    pending: bool,
    fail_at: Option<usize>,
    consumed: usize,
    polls: Rc<Cell<usize>>,
}
impl Source {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: Bytes::copy_from_slice(bytes),
            quantum: 2,
            pending: true,
            fail_at: None,
            consumed: 0,
            polls: Rc::default(),
        }
    }
    fn ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.polls.set(self.polls.get() + 1);
        if self.fail_at.is_some_and(|at| self.consumed >= at) {
            return Poll::Ready(Err(io::Error::other("source failure")));
        }
        if self.pending {
            self.pending = false;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.pending = true;
        Poll::Ready(Ok(()))
    }
}
impl ReadHost for Source {
    type Error = io::Error;
    fn poll_read<'visit, B: BufMut + ?Sized>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
        mut out: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        let this = this.get_mut();
        std::task::ready!(this.ready(cx))?;
        let count = out
            .remaining_mut()
            .min(this.quantum)
            .min(this.bytes.len())
            .min(this.fail_at.map_or(usize::MAX, |at| at - this.consumed));
        BufMut::put(&mut out, this.bytes.split_to(count));
        this.consumed += count;
        Poll::Ready(Ok(count))
    }
}
struct ChunkSource(Source);
impl ReadHost for ChunkSource {
    type Error = io::Error;
    fn poll_read<'visit, B: BufMut + ?Sized>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        if this.0.fail_at.is_some_and(|at| this.0.consumed >= at) {
            return Poll::Ready(Err(io::Error::other("source failure")));
        }
        poll_read_from_chunk(this, cx, out)
    }
}
impl ChunkHost for ChunkSource {
    type Chunk = Bytes;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Bytes>, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        let this = &mut this.get_mut().0;
        std::task::ready!(this.ready(cx))?;
        let count = maximum
            .get()
            .min(this.quantum)
            .min(this.bytes.len())
            .min(this.fail_at.map_or(usize::MAX, |at| at - this.consumed));
        if count == 0 {
            return Poll::Ready(Ok(None));
        }
        this.consumed += count;
        Poll::Ready(Ok(Some(this.bytes.split_to(count))))
    }
}
struct Sink {
    bytes: Vec<u8>,
    pending: bool,
    fail_at: Option<usize>,
    zero: bool,
}
impl Sink {
    fn new() -> Self {
        Self {
            bytes: vec![],
            pending: true,
            fail_at: None,
            zero: false,
        }
    }
}
impl WriteHost for Sink {
    type Error = io::Error;
    fn poll_write<'visit, B: Buf + ?Sized>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
        input: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        let this = this.get_mut();
        if this.fail_at.is_some_and(|at| this.bytes.len() >= at) {
            return Poll::Ready(Err(io::Error::other("sink failure")));
        }
        if this.pending {
            this.pending = false;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.pending = true;
        if this.zero {
            return Poll::Ready(Ok(0));
        }
        let count = 2.min(input.remaining());
        this.bytes.put(input.take(count));
        Poll::Ready(Ok(count))
    }
}

#[test]
fn exact_prechecks_capacity_and_limits_source() {
    let mut source = Source::new(b"abcdef");
    let mut small = [0; 2];
    let error = run(BoundCall::new(
        Pin::new(&mut source),
        ReadExact::new(&mut small.as_mut_slice(), 3),
    ))
    .unwrap_err();
    assert!(matches!(
        error,
        ReadExactError::InsufficientCapacity {
            source: InsufficientCapacity {
                required: 3,
                available: 2,
                completed: 0
            }
        }
    ));
    assert_eq!(source.polls.get(), 0);
    let mut large = [0; 8];
    let mut out = large.as_mut_slice();
    run(BoundCall::new(
        Pin::new(&mut source),
        ReadExact::new(&mut out, 3),
    ))
    .unwrap();
    assert_eq!(out.len(), 5);
    assert_eq!(&large[..3], b"abc");
    assert_eq!(source.bytes, b"def"[..]);
}

#[test]
fn exact_errors_preserve_prefix_and_source_chain() {
    let mut source = Source::new(b"ab");
    let mut out = vec![];
    let error = run(BoundCall::new(
        Pin::new(&mut source),
        ReadExact::new(&mut out, 4),
    ))
    .unwrap_err();
    assert!(matches!(
        error,
        ReadExactError::Incomplete {
            source: Incomplete {
                expected: 4,
                completed: 2
            }
        }
    ));
    assert_eq!(out, b"ab");
    source = Source::new(b"abcdef");
    source.fail_at = Some(2);
    let error = run(BoundCall::new(Pin::new(&mut source), ReadArray::<4>::new())).unwrap_err();
    assert_eq!(error.completed(), 2);
    assert_eq!(error.host_error().unwrap().kind(), io::ErrorKind::Other);
    assert_eq!(
        std::error::Error::source(&error).unwrap().to_string(),
        "source failure"
    );
}

#[test]
fn read_to_end_appends_and_requires_confirmed_eof() {
    let mut source = Source::new(b"abcd");
    let mut out = b"prefix".to_vec();
    assert_eq!(
        run(BoundCall::new(
            Pin::new(&mut source),
            ReadToEnd::new(&mut out)
        ))
        .unwrap(),
        4
    );
    assert_eq!(out, b"prefixabcd");
    let mut source = Source::new(b"abcd");
    let mut bytes = [0; 4];
    let error = run(BoundCall::new(
        Pin::new(&mut source),
        ReadToEnd::new(&mut bytes.as_mut_slice()),
    ))
    .unwrap_err();
    assert!(matches!(
        error,
        ReadToEndError::InsufficientCapacity {
            source: InsufficientCapacity { completed: 4, .. }
        }
    ));
    assert_eq!(source.consumed, 4);
    assert_eq!(source.polls.get(), 4); // no EOF probe after filling the target
}

#[test]
fn chunks_are_lazy_bounded_and_share_the_fill_cursor() {
    let mut source = ChunkSource(Source::new(b"abcdefgh"));
    let polls = source.0.polls.clone();
    assert_eq!(
        run(BoundCall::new(Pin::new(&mut source), ReadArray::<1>::new())).unwrap(),
        *b"a"
    );
    let mut bound = pin!(BoundCall::new(
        Pin::new(&mut source),
        ReadChunks::new(4, nz(3))
    ));
    for expected in [b"bc", b"de"] {
        let before = polls.get();
        match run(bound.as_mut().next()).unwrap() {
            CoroutineState::Yielded(bytes) => assert_eq!(bytes, expected[..]),
            _ => panic!(),
        }
        assert_eq!(polls.get(), before + 2); // one Pending, one chunk; no prefetch
    }
    let before = polls.get();
    assert!(matches!(
        run(bound.as_mut().next()),
        Some(CoroutineState::Complete(Ok(4)))
    ));
    assert_eq!(polls.get(), before);
    assert!(run(bound.as_mut().next()).is_none());
}

#[test]
fn exact_chunks_deliver_prefix_before_eof_or_error() {
    for fail in [false, true] {
        let mut source = ChunkSource(Source::new(b"ab"));
        if fail {
            source.0.fail_at = Some(2);
        }
        let mut bound = pin!(BoundCall::new(
            Pin::new(&mut source),
            ReadChunksExact::new(3, nz(2))
        ));
        assert!(matches!(
            run(bound.as_mut().next()),
            Some(CoroutineState::Yielded(_))
        ));
        let Some(CoroutineState::Complete(Err(error))) = run(bound.as_mut().next()) else {
            panic!()
        };
        assert_eq!(error.completed(), 2);
        assert_eq!(error.host_error().is_some(), fail);
    }
}

#[test]
fn write_ownership_borrowing_and_partial_errors() {
    let mut sink = Sink::new();
    let result = run(BoundCall::new(
        Pin::new(&mut sink),
        Write::new(Bytes::from_static(b"abcd")),
    ));
    assert_eq!(result.result.unwrap(), 2);
    assert_eq!(result.input, b"cd"[..]);
    let mut input = Bytes::from_static(b"efghij");
    sink.fail_at = Some(4);
    let result = run(BoundCall::new(
        Pin::new(&mut sink),
        WriteAll::new(&mut input),
    ));
    let error = result.result.unwrap_err();
    assert_eq!(error.completed(), 2);
    assert!(error.host_error().is_some());
    assert_eq!(input, b"ghij"[..]);
    assert_eq!(sink.bytes, b"abef");
    let mut sink = Sink::new();
    sink.zero = true;
    let result = run(BoundCall::new(
        Pin::new(&mut sink),
        WriteAll::new(Bytes::from_static(b"x")),
    ));
    assert!(matches!(
        result.result.unwrap_err(),
        WriteAllError::WriteZero {
            source: WriteZero { completed: 0 }
        }
    ));
    assert_eq!(result.input, b"x"[..]);
}

#[test]
fn segmented_and_unsized_buffers() {
    let mut sink = Sink::new();
    let mut input = (&b"ab"[..]).chain(&b"cde"[..]);
    let input: &mut dyn Buf = &mut input;
    let result = run(BoundCall::new(Pin::new(&mut sink), WriteAll::new(input)));
    assert_eq!(result.result.unwrap(), 5);
    assert_eq!(sink.bytes, b"abcde");
    let mut source = Source::new(b"abcde");
    let mut left = [0; 2];
    let mut right = [0; 3];
    let mut chain = left.as_mut_slice().chain_mut(right.as_mut_slice());
    let out: &mut dyn BufMut = &mut chain;
    run(BoundCall::new(
        Pin::new(&mut source),
        ReadExact::new(out, 5),
    ))
    .unwrap();
    assert_eq!(left, *b"ab");
    assert_eq!(right, *b"cde");
}

#[test]
fn fill_to_chunk_retains_storage_and_respects_changed_maximum() {
    let mut source = Source::new(b"abcdef");
    let mut storage = None;
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    assert!(
        poll_chunk_from_read(
            Pin::new(&mut source),
            &mut cx,
            nz(4),
            &mut storage,
            BytesMut::with_capacity,
            BytesMut::freeze
        )
        .is_pending()
    );
    let address = storage.as_ref().unwrap().as_ptr();
    assert_eq!(source.consumed, 0);
    let result = poll_chunk_from_read(
        Pin::new(&mut source),
        &mut cx,
        nz(1),
        &mut storage,
        |_| panic!("must reuse storage"),
        BytesMut::freeze,
    );
    let Poll::Ready(Ok(Some(chunk))) = result else {
        panic!()
    };
    assert_eq!(chunk, b"a"[..]);
    assert_eq!(chunk.as_ptr(), address);
    assert!(storage.is_none());
}

#[call]
async fn local_codec<H: ReadHost>(
    io: nitori_call::Receiver<H>,
) -> Result<[u8; 3], nitori_io::calls::ReadExactError<H::Error>> {
    let [first] = io.read_array::<1>().await?;
    let mut rest = [0; 2];
    let mut destination = rest.as_mut_slice();
    io.read_exact(&mut destination, 2).await?;
    Ok([first, rest[0], rest[1]])
}
#[call]
async fn number_codec<H: ReadHost>(
    io: nitori_call::Receiver<H>,
) -> Result<u32, nitori_io::calls::ReadLeError<H::Error>> {
    io.read_le::<u32>().await
}
#[test]
fn family_calls_capture_local_buffers_across_pending() {
    let mut source = Source::new(b"abc");
    assert_eq!(
        run(BoundCall::new(
            Pin::new(&mut source),
            LocalCodec::<Source>::new()
        ))
        .unwrap(),
        *b"abc"
    );
    let mut source = Source::new(&0x12345678u32.to_le_bytes());
    assert_eq!(
        run(BoundCall::new(
            Pin::new(&mut source),
            NumberCodec::<Source>::new()
        ))
        .unwrap(),
        0x12345678
    );
}

#[test]
fn cancellation_keeps_completed_prefix() {
    let mut source = Source::new(b"abcd");
    source.pending = false;
    let mut out = vec![];
    {
        let mut bound = pin!(BoundCall::new(
            Pin::new(&mut source),
            ReadExact::new(&mut out, 4)
        ));
        assert!(
            bound
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }
    assert_eq!(out, b"ab");
    assert_eq!(source.bytes, b"cd"[..]);
    let mut sink = Sink::new();
    sink.pending = false;
    let mut input = Bytes::from_static(b"abcd");
    {
        let mut bound = pin!(BoundCall::new(
            Pin::new(&mut sink),
            WriteAll::new(&mut input)
        ));
        assert!(
            bound
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }
    assert_eq!(input, b"cd"[..]);
    assert_eq!(sink.bytes, b"ab");
}

#[test]
fn zero_composites_are_noops_but_basic_requests_reach_host() {
    let mut source = Source::new(b"");
    source.fail_at = Some(0);
    assert_eq!(
        run(BoundCall::new(Pin::new(&mut source), ReadArray::<0>::new())).unwrap(),
        []
    );
    assert!(
        run(BoundCall::new(
            Pin::new(&mut source),
            Read::new(&mut [0u8; 0].as_mut_slice())
        ))
        .is_err()
    );
    let mut sink = Sink::new();
    sink.fail_at = Some(0);
    assert_eq!(
        run(BoundCall::new(Pin::new(&mut sink), WriteAll::new(&b""[..])))
            .result
            .unwrap(),
        0
    );
    assert!(
        run(BoundCall::new(Pin::new(&mut sink), Write::new(&b""[..])))
            .result
            .is_err()
    );
}

#[test]
fn endian_numbers_preserve_bits() {
    macro_rules! check {
        ($ty:ty, $value:expr) => {{
            let value: $ty = $value;
            let mut source = Source::new(&value.to_le_bytes());
            let actual = run(BoundCall::new(Pin::new(&mut source), ReadLe::<$ty>::new())).unwrap();
            assert_eq!(actual.to_le_bytes(), value.to_le_bytes());
            let mut source = Source::new(&value.to_be_bytes());
            let actual = run(BoundCall::new(Pin::new(&mut source), ReadBe::<$ty>::new())).unwrap();
            assert_eq!(actual.to_be_bytes(), value.to_be_bytes());
        }};
    }
    check!(u8, 0xa5);
    check!(u16, 0xa123);
    check!(u32, 0xa1234567);
    check!(u64, 0xf123456789abcdef);
    check!(u128, u128::MAX - 7);
    check!(usize, usize::MAX - 123);
    check!(i8, -7);
    check!(i16, -1234);
    check!(i32, -1234567);
    check!(i64, i64::MIN + 123);
    check!(i128, i128::MIN + 456);
    check!(isize, isize::MIN + 7);
    check!(f32, f32::from_bits(0x7fc01234));
    check!(f64, f64::from_bits(0x7ff8000000001234));
}

#[test]
fn basic_calls_and_chunk_termination() {
    let mut source = ChunkSource(Source::new(b"abcde"));
    let mut out = vec![];
    assert_eq!(
        run(BoundCall::new(Pin::new(&mut source), Read::new(&mut out))).unwrap(),
        2
    );
    assert_eq!(out, b"ab");
    assert_eq!(
        run(BoundCall::new(Pin::new(&mut source), ReadChunk::new(nz(1))))
            .unwrap()
            .unwrap(),
        b"c"[..]
    );
    run(BoundCall::new(
        Pin::new(&mut source),
        ReadChunksExact::new(2, nz(1)),
    ))
    .unwrap();
    assert!(
        run(BoundCall::new(Pin::new(&mut source), ReadChunk::new(nz(1))))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        run(BoundCall::new(
            Pin::new(&mut source),
            ReadChunks::new(100, nz(5))
        ))
        .unwrap(),
        0
    );
    let before = source.0.polls.get();
    assert_eq!(
        run(BoundCall::new(
            Pin::new(&mut source),
            ReadChunks::new(0, nz(5))
        ))
        .unwrap(),
        0
    );
    assert_eq!(source.0.polls.get(), before);
}

#[test]
fn fill_to_chunk_clears_storage_on_eof_error_and_cancel() {
    for fail in [false, true] {
        let mut source = Source::new(b"");
        source.pending = false;
        if fail {
            source.fail_at = Some(0);
        }
        let mut storage = Some(BytesMut::with_capacity(2));
        let result = poll_chunk_from_read(
            Pin::new(&mut source),
            &mut Context::from_waker(Waker::noop()),
            nz(2),
            &mut storage,
            |_| panic!(),
            BytesMut::freeze,
        );
        if fail {
            assert!(matches!(result, Poll::Ready(Err(_))));
        } else {
            assert!(matches!(result, Poll::Ready(Ok(None))));
        }
        assert!(storage.is_none());
    }
    let mut source = Source::new(b"abc");
    let mut storage = None;
    assert!(
        poll_chunk_from_read(
            Pin::new(&mut source),
            &mut Context::from_waker(Waker::noop()),
            nz(2),
            &mut storage,
            BytesMut::with_capacity,
            BytesMut::freeze
        )
        .is_pending()
    );
    drop(storage);
    assert_eq!(
        run(BoundCall::new(Pin::new(&mut source), ReadArray::<3>::new())).unwrap(),
        *b"abc"
    );
}

#[test]
fn read_to_end_preserves_partial_host_failure() {
    let mut source = Source::new(b"abcd");
    source.fail_at = Some(2);
    let mut out = b"prefix".to_vec();
    let error = run(BoundCall::new(
        Pin::new(&mut source),
        ReadToEnd::new(&mut out),
    ))
    .unwrap_err();
    assert_eq!(error.completed(), 2);
    assert!(error.host_error().is_some());
    assert_eq!(out, b"prefixab");
}

pin_project_lite::pin_project! {
    struct PinnedHost {
        inner: Source,
        #[pin]
        marker: std::marker::PhantomPinned,
    }
}
impl ReadHost for PinnedHost {
    type Error = io::Error;
    fn poll_read<'visit, B: BufMut + ?Sized>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        Pin::new(this.project().inner).poll_read(cx, destination)
    }
}
#[test]
fn host_does_not_need_unpin() {
    let host = PinnedHost {
        inner: Source::new(b"abc"),
        marker: std::marker::PhantomPinned,
    };
    let mut host = pin!(host);
    assert_eq!(
        run(BoundCall::new(host.as_mut(), ReadArray::<3>::new())).unwrap(),
        *b"abc"
    );
}
struct BorrowedErrorHost<'a>(&'a str);
impl<'a> ReadHost for BorrowedErrorHost<'a> {
    type Error = &'a str;
    fn poll_read<'visit, B: BufMut + ?Sized>(
        host: Pin<&mut Self::Host<'visit>>,
        _: &mut Context<'_>,
        _: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        Poll::Ready(Err(this.0))
    }
}
#[test]
fn host_errors_need_neither_error_trait_nor_static_lifetime() {
    let message = String::from("borrowed");
    let mut host = BorrowedErrorHost(&message);
    let error = run(BoundCall::new(Pin::new(&mut host), ReadArray::<1>::new())).unwrap_err();
    assert_eq!(*error.host_error().unwrap(), message);
}

#[call]
async fn borrowed_write<H: WriteHost>(
    io: nitori_call::Receiver<H>,
) -> Result<usize, nitori_io::calls::WriteAllError<H::Error>> {
    let mut input = Bytes::from_static(b"abc");
    let returned = io.write_all(&mut input).await;
    returned.result
}
#[test]
fn family_write_borrows_local_input() {
    let mut host = Sink::new();
    assert_eq!(
        run(BoundCall::new(
            Pin::new(&mut host),
            BorrowedWrite::<Sink>::new()
        ))
        .unwrap(),
        3
    );
    assert_eq!(host.bytes, b"abc");
}

// No diagnostic traits: borrowing a host failure must not restrict IO calls.
struct RawFailure<'a>(&'a str);
struct RawHost<'a> {
    failure: Option<&'a str>,
}
impl<'a> ReadHost for RawHost<'a> {
    type Error = RawFailure<'a>;
    fn poll_read<'visit, B: BufMut + ?Sized>(
        host: Pin<&mut Self::Host<'visit>>,
        _: &mut Context<'_>,
        _: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        Poll::Ready(this.failure.map_or(Ok(0), |text| Err(RawFailure(text))))
    }
}
impl ChunkHost for RawHost<'_> {
    type Chunk = Bytes;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::Host<'visit>>,
        _: &mut Context<'_>,
        _: NonZeroUsize,
    ) -> Poll<Result<Option<Bytes>, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        Poll::Ready(this.failure.map_or(Ok(None), |text| Err(RawFailure(text))))
    }
}
impl<'a> WriteHost for RawHost<'a> {
    type Error = RawFailure<'a>;
    fn poll_write<'visit, B: Buf + ?Sized>(
        host: Pin<&mut Self::Host<'visit>>,
        _: &mut Context<'_>,
        _: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        Poll::Ready(this.failure.map_or(Ok(0), |text| Err(RawFailure(text))))
    }
}

#[test]
fn operation_errors_accept_borrowed_payloads_without_diagnostic_traits() {
    let message = String::from("raw failure");
    let mut host = RawHost {
        failure: Some(&message),
    };
    macro_rules! check_read {
        ($call:expr) => {
            let error = run(BoundCall::new(Pin::new(&mut host), $call))
                .err()
                .unwrap();
            assert_eq!(error.host_error().unwrap().0, message);
            assert_eq!(error.completed(), 0);
            assert_eq!(error.to_string(), "read failed after 0 bytes");
            assert!(snafu::ErrorCompat::backtrace(&error).is_none());
        };
    }
    check_read!(ReadArray::<2>::new());
    check_read!(ReadLe::<u16>::new());
    check_read!(ReadBe::<u16>::new());
    let mut buffer = Vec::new();
    check_read!(ReadExact::new(&mut buffer, 2));
    check_read!(ReadToEnd::new(&mut buffer));

    let mut chunks = pin!(BoundCall::new(
        Pin::new(&mut host),
        ReadChunks::new(2, nz(2))
    ));
    let Some(CoroutineState::Complete(Err(error))) = run(chunks.as_mut().next()) else {
        panic!("expected chunk host failure");
    };
    assert_eq!(error.host_error().0, message);
    assert_eq!(error.to_string(), "read failed after 0 bytes");

    let mut exact = pin!(BoundCall::new(
        Pin::new(&mut host),
        ReadChunksExact::new(2, nz(2))
    ));
    let Some(CoroutineState::Complete(Err(error))) = run(exact.as_mut().next()) else {
        panic!("expected exact chunk host failure");
    };
    assert_eq!(error.host_error().unwrap().0, message);
    assert_eq!(error.to_string(), "read failed after 0 bytes");

    let returned = run(BoundCall::new(
        Pin::new(&mut host),
        WriteAll::new(Bytes::from_static(b"x")),
    ));
    let error = returned.result.err().unwrap();
    assert_eq!(error.host_error().unwrap().0, message);
    assert_eq!(error.to_string(), "write failed after 0 bytes");
    assert_eq!(returned.input, b"x"[..]);
}

#[test]
fn primitive_failures_do_not_require_host_diagnostics() {
    use nitori_io::calls::{ReadArrayError, ReadBeError, ReadLeError};
    let mut host = RawHost { failure: None };
    macro_rules! incomplete {
        ($call:expr, $error:ident) => {
            let error = run(BoundCall::new(Pin::new(&mut host), $call))
                .err()
                .unwrap();
            assert_eq!(error.to_string(), "read incomplete after 0 of 2 bytes");
            match error {
                $error::Incomplete {
                    source:
                        Incomplete {
                            expected: 2,
                            completed: 0,
                        },
                } => {}
                $error::Incomplete { .. } | $error::Host { .. } => {
                    panic!("expected incomplete read")
                }
            }
        };
    }
    incomplete!(ReadArray::<2>::new(), ReadArrayError);
    incomplete!(ReadLe::<u16>::new(), ReadLeError);
    incomplete!(ReadBe::<u16>::new(), ReadBeError);
    let mut empty: &mut [u8] = &mut [];
    let error = run(BoundCall::new(
        Pin::new(&mut host),
        ReadExact::new(&mut empty, 1),
    ))
    .err()
    .unwrap();
    assert_eq!(error.completed(), 0);
    assert!(matches!(error, ReadExactError::InsufficientCapacity { .. }));
    let returned = run(BoundCall::new(
        Pin::new(&mut host),
        WriteAll::new(Bytes::from_static(b"x")),
    ));
    let error = returned.result.err().unwrap();
    assert!(matches!(
        error,
        WriteAllError::WriteZero {
            source: WriteZero { completed: 0 }
        }
    ));
    assert_eq!(error.to_string(), "write made no progress after 0 bytes");
}

#[test]
fn numeric_errors_preserve_progress_and_original_source() {
    use nitori_io::calls::{ReadBeError, ReadLeError};
    macro_rules! check {
        ($call:expr, $error:ident) => {
            let mut source = Source::new(b"abcd");
            source.fail_at = Some(2);
            let error = run(BoundCall::new(Pin::new(&mut source), $call)).unwrap_err();
            assert_eq!(error.completed(), 2);
            let original = std::error::Error::source(&error).unwrap();
            assert!(std::ptr::eq(
                original.downcast_ref::<io::Error>().unwrap(),
                error.host_error().unwrap()
            ));
            assert_eq!(snafu::ErrorCompat::iter_chain(&error).count(), 2);
            match error {
                $error::Host { completed: 2, .. } => {}
                $error::Host { .. } | $error::Incomplete { .. } => {
                    panic!("expected partial host failure")
                }
            }
        };
    }
    check!(ReadLe::<u32>::new(), ReadLeError);
    check!(ReadBe::<u32>::new(), ReadBeError);
}

nitori_call::family_host!(impl [] for Source);

nitori_call::family_host!(impl [] for ChunkSource);

nitori_call::family_host!(impl [] for Sink);

nitori_call::family_host!(impl [] for PinnedHost);

nitori_call::family_host!(impl ['a] for BorrowedErrorHost<'a>);

nitori_call::family_host!(impl ['a] for RawHost<'a>);

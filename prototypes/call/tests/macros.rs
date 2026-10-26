#![feature(impl_trait_in_assoc_type, yield_expr)]
use bytes::BufMut;
use futures_core::Stream;
use nitori_call_prototype::{
    Call, CallPoll, Compose, Direct, Receiver, ReceiverAccess, ReceiverFamily, ReceiverSlot, Start,
    ViewLoan, call, capability, direct_receiver,
};
use std::{
    collections::VecDeque,
    convert::Infallible,
    future::poll_fn,
    marker::PhantomData,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker, ready},
};

/// The same contract as `nitori_io::Read`; the macro adds its operations,
/// here in the default module `read`.
#[capability(sync)]
pub trait Read: ReceiverFamily {
    type Error;
    fn poll_read<'visit, Output: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit;
}
impl Read for Direct<&[u8]> {
    type Error = Infallible;
    fn poll_read<'visit, Output: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'visit,
    {
        let source = host.get_mut().0.as_mut().get_mut();
        let count = source.len().min(destination.remaining_mut());
        destination.put_slice(&source[..count]);
        *source = &source[count..];
        Poll::Ready(Ok(count))
    }
}

#[derive(Default)]
struct Feed {
    data: VecDeque<u8>,
    waker: Option<Waker>,
}
direct_receiver!(impl [] for Feed);
impl Read for Direct<Feed> {
    type Error = Infallible;
    fn poll_read<'visit, Output: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'visit,
    {
        let feed = host.get_mut().0.as_mut().get_mut();
        if feed.data.is_empty() && destination.has_remaining_mut() {
            feed.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let count = feed.data.len().min(destination.remaining_mut());
        for byte in feed.data.drain(..count) {
            destination.put_u8(byte);
        }
        Poll::Ready(Ok(count))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Incomplete {
    completed: usize,
}

#[call(sync)]
async fn read_array<const LENGTH: usize>(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
) -> Result<[u8; LENGTH], Incomplete> {
    let mut array = [0; LENGTH];
    let mut filled = 0;
    while filled < LENGTH {
        // A generated primitive operation.
        match io.read(&mut &mut array[filled..]).await {
            Ok(0) => return Err(Incomplete { completed: filled }),
            Ok(count) => filled += count,
            Err(never) => match never {},
        }
    }
    Ok(array)
}

#[call(sync)]
async fn read_exact<A>(io: &mut A, output: &mut Vec<u8>, length: usize) -> Result<(), Incomplete>
where
    A: ReceiverAccess<Family: Read<Error = Infallible>>,
{
    let mut completed = 0;
    while completed < length {
        match io.read(&mut output.limit(length - completed)).await {
            Ok(0) => return Err(Incomplete { completed }),
            Ok(count) => completed += count,
            Err(never) => match never {},
        }
    }
    Ok(())
}

#[call(sync)]
async fn read_record(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    output: &mut Vec<u8>,
) -> Result<usize, Incomplete> {
    // Generated methods for derived operations, with turbofish on the method.
    let [length] = io.read_array::<1>().await?;
    io.read_exact(output, length.into()).await?;
    Ok(length.into())
}

#[test]
fn a_host_runs_a_derived_operation_synchronously() {
    let mut input = b"\x03cat\x03dog".as_slice();
    let mut output = Vec::new();
    assert_eq!(input.sync_read_record_unpin(&mut output), Ok(3));
    assert_eq!(output, b"cat");
    assert_eq!(input, b"\x03dog");
}

#[test]
fn a_host_runs_a_primitive_operation() {
    let mut input = b"abc".as_slice();
    let mut output = [0; 2];
    assert_eq!(input.sync_read_unpin(&mut output.as_mut_slice()), Ok(2));
    assert_eq!(output, *b"ab");
}

#[test]
fn a_bound_operation_waits_and_resumes() {
    let mut feed = Feed::default();
    feed.data.extend(*b"\x02h");
    let mut output = Vec::new();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut operation = pin!(feed.read_record_unpin(&mut output));
        assert!(operation.as_mut().poll(&mut cx).is_pending());
    }
    assert!(feed.waker.is_some());
    feed.data.extend(*b"i");
    // A new binding starts a new call; the first call already consumed "\x02h".
    let mut rest = Vec::new();
    {
        let mut operation = pin!(feed.read_exact_unpin(&mut rest, 1));
        assert_eq!(operation.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    assert_eq!(output, b"h");
    assert_eq!(rest, b"i");
}

/// A primitive operation is also a root operation, and its future is the
/// operation struct with the access filled in.
#[test]
fn a_primitive_operation_is_a_root_and_names_its_future() {
    fn started<'a, 'b, A: ReceiverAccess<Family: Read>>(
        io: &'a mut A,
        destination: &'a mut &'b mut [u8],
    ) -> read::Read<'a, &'b mut [u8], &'a mut A> {
        io.read(destination)
    }
    let _ = started::<ReceiverSlot<'_, Direct<&[u8]>>>;

    let mut input = b"xyz".as_slice();
    let mut output = [0; 2];
    let mut destination = output.as_mut_slice();
    let mut call = pin!(Call::<Direct<&[u8]>, _>::new(read::Read::new(
        &mut destination
    )));
    let result = call.as_mut().poll_with(
        Some(pin!(Pin::new(&mut input).view())),
        &mut Context::from_waker(Waker::noop()),
    );
    assert!(matches!(result, CallPoll::Ready(ControlFlow::Break(Ok(2)))));
    assert_eq!(output, *b"xy");
}

/// A byte budget owned by the operation.
struct ReadLimit(usize);
struct Limited<F>(PhantomData<fn(F) -> F>);
struct LimitedView<'v, F: ReceiverFamily> {
    inner: ViewLoan<'v, F>,
    remaining: &'v mut usize,
}
impl<F: ReceiverFamily> ReceiverFamily for Limited<F> {
    type ReceiverView<'v>
        = LimitedView<'v, F>
    where
        Self: 'v;
}
impl<F: ReceiverFamily> Compose<F> for ReadLimit {
    type Family = Limited<F>;
    fn compose<'v, 'p>(
        self: Pin<&'v mut Self>,
        inner: Pin<&'v mut F::ReceiverView<'p>>,
    ) -> LimitedView<'v, F>
    where
        F: 'p,
        Self::Family: 'v,
        'p: 'v,
    {
        LimitedView {
            inner: ViewLoan::new(inner),
            remaining: &mut self.get_mut().0,
        }
    }
}
impl<F: Read> Read for Limited<F> {
    type Error = F::Error;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'v,
    {
        let view = host.get_mut();
        if *view.remaining == 0 {
            return Poll::Ready(Ok(0));
        }
        let count = ready!(view.inner.with(|access| {
            F::poll_read(access.into_pin(), cx, &mut out.limit(*view.remaining))
        }))?;
        *view.remaining -= count;
        Poll::Ready(Ok(count))
    }
}

#[call(sync)]
async fn read_limited_word(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
) -> Result<[u8; 2], Incomplete> {
    let [length] = io.read_array::<1>().await?;
    io.compose(ReadLimit(length.into())).read_array::<2>().await
}

#[test]
fn generated_methods_apply_to_composed_accesses() {
    let mut input = b"\x01A\x02BC".as_slice();
    assert_eq!(
        input.sync_read_limited_word_unpin(),
        Err(Incomplete { completed: 1 })
    );
    assert_eq!(input, b"\x02BC");
    assert_eq!(input.sync_read_limited_word_unpin(), Ok(*b"BC"));
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn a_call_of_a_generated_operation_over_a_generic_family_is_send() {
    fn build<F: Read<Error = Infallible>>(output: &mut Vec<u8>) {
        assert_send(&Call::<F, _>::new(ReadRecord::new(output)));
        assert_send(&Call::<F, _>::new(ReadLimitedWord::new()));
    }
    build::<Direct<&[u8]>>(&mut Vec::new());
}

/// The future of a derived operation is nameable through `Start`.
type RecordFuture<'a, A> = <ReadRecord<'a> as Start<'static, &'a mut A>>::Fut;
fn record_future<'a, A: ReceiverAccess<Family: Read<Error = Infallible>>>(
    io: &'a mut A,
    output: &'a mut Vec<u8>,
) -> RecordFuture<'a, A> {
    io.read_record(output)
}

#[test]
fn a_derived_future_is_nameable() {
    let _ = record_future::<ReceiverSlot<'_, Direct<&[u8]>>>;
}

/// Operations in a module chosen by the attribute.
#[capability(module = writing)]
pub trait Write: ReceiverFamily {
    fn poll_write<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        source: &[u8],
    ) -> Poll<usize>
    where
        Self: 'visit;
}
impl Write for Direct<Vec<u8>> {
    fn poll_write<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        source: &[u8],
    ) -> Poll<usize>
    where
        Self: 'visit,
    {
        host.get_mut()
            .0
            .as_mut()
            .get_mut()
            .extend_from_slice(source);
        Poll::Ready(source.len())
    }
}

/// Operations in the current module.
#[capability(module = self)]
pub trait Clear: ReceiverFamily {
    fn poll_clear_all<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
    ) -> Poll<()>
    where
        Self: 'visit;
}
impl Clear for Direct<Vec<u8>> {
    fn poll_clear_all<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
    ) -> Poll<()>
    where
        Self: 'visit,
    {
        host.get_mut().0.as_mut().get_mut().clear();
        Poll::Ready(())
    }
}

#[call(module = rewriting)]
async fn rewrite(io: &mut impl ReceiverAccess<Family: Write + Clear>, source: &[u8]) -> usize {
    io.clear_all().await;
    io.write(source).await
}

#[test]
fn the_attribute_chooses_the_module() {
    let mut sink = b"old".to_vec();
    let mut call = pin!(Call::<Direct<Vec<u8>>, _>::new(rewriting::Rewrite::new(
        b"new"
    )));
    let result = call.as_mut().poll_with(
        Some(pin!(Pin::new(&mut sink).view())),
        &mut Context::from_waker(Waker::noop()),
    );
    assert!(matches!(result, CallPoll::Ready(ControlFlow::Break(3))));
    assert_eq!(sink, b"new");
    let _: writing::Write<'_> = writing::Write::new(b"");
    let _: ClearAll = ClearAll::new();
}

/// An operation that emits: each `yield` is an item, the return value is the
/// final value.
#[call(sync, yields = Vec<u8>)]
async fn read_chunks(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    size: usize,
) -> usize {
    let mut total = 0;
    loop {
        let mut chunk = Vec::new();
        match io.read(&mut (&mut chunk).limit(size)).await {
            Ok(0) => return total,
            Ok(count) => total += count,
            Err(never) => match never {},
        }
        yield chunk;
    }
}

/// A parent with the same item type forwards the child's items.
#[call(yields = Vec<u8>)]
async fn read_framed_chunks<A>(io: &mut A) -> Result<usize, Incomplete>
where
    A: ReceiverAccess<Family: Read<Error = Infallible>>,
{
    let [size] = io.read_array::<1>().await?;
    yield vec![size];
    let mut chunks = pin!(io.read_chunks(size.into()));
    loop {
        match poll_fn(|cx| chunks.as_mut().poll_next(cx)).await {
            Some(ControlFlow::Continue(chunk)) => yield chunk,
            Some(ControlFlow::Break(total)) => return Ok(total),
            None => unreachable!("the final value ends the stream"),
        }
    }
}

type Memory = Direct<&'static [u8]>;
type Events<Op> = (
    Vec<<Op as nitori_call_prototype::Operation<Memory>>::Yield>,
    <Op as nitori_call_prototype::Operation<Memory>>::Output,
);

fn events<Op>(
    call: Pin<&mut Call<Direct<&'static [u8]>, Op>>,
    input: &mut &'static [u8],
) -> Events<Op>
where
    Op: nitori_call_prototype::Operation<Direct<&'static [u8]>>
        + for<'s> Start<'s, ReceiverSlot<'s, Direct<&'static [u8]>>>,
{
    let mut call = call;
    let mut items = Vec::new();
    loop {
        match call.as_mut().poll_with(
            Some(pin!(Pin::new(&mut *input).view())),
            &mut Context::from_waker(Waker::noop()),
        ) {
            CallPoll::Ready(ControlFlow::Continue(item)) => items.push(item),
            CallPoll::Ready(ControlFlow::Break(result)) => return (items, result),
            CallPoll::Pending => panic!("memory input does not wait"),
            CallPoll::NeedsView => unreachable!("a call given a view asked for one"),
        }
    }
}

#[test]
fn a_generated_operation_yields_items_then_its_final_value() {
    let mut input: &'static [u8] = b"\x02abcde";
    let mut call = pin!(Call::new(ReadFramedChunks::new()));
    let (items, result) = events(call.as_mut(), &mut input);
    assert_eq!(
        items,
        [vec![2], b"ab".to_vec(), b"cd".to_vec(), b"e".to_vec()]
    );
    assert_eq!(result, Ok(5));
}

#[test]
fn a_host_runs_an_emitting_operation_as_a_stream_or_a_future() {
    let mut input = b"abcde".as_slice();
    let mut execution = pin!(input.read_chunks_unpin(2));
    let mut cx = Context::from_waker(Waker::noop());
    let mut items = Vec::new();
    let total = loop {
        match execution.as_mut().poll_next(&mut cx) {
            Poll::Ready(Some(ControlFlow::Continue(chunk))) => items.push(chunk),
            Poll::Ready(Some(ControlFlow::Break(total))) => break total,
            _ => panic!("memory input does not wait"),
        }
    };
    assert_eq!((items.len(), total), (3, 5));
    assert_eq!(execution.as_mut().poll_next(&mut cx), Poll::Ready(None));

    // As a future, the same operation discards its items.
    let mut input = b"abcde".as_slice();
    assert_eq!(input.sync_read_chunks_unpin(2), 5);
}

#[test]
fn a_host_runs_an_operation_without_items_as_a_stream() {
    let mut input = b"\x01z".as_slice();
    let mut output = Vec::new();
    let mut execution = pin!(input.read_record_unpin(&mut output));
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(
        execution.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(ControlFlow::Break(Ok(1))))
    );
    assert_eq!(execution.as_mut().poll_next(&mut cx), Poll::Ready(None));
}

#[test]
fn calls_of_emitting_operations_are_send() {
    fn build<F: Read<Error = Infallible>>() {
        assert_send(&Call::<F, _>::new(ReadChunks::new(1)));
        assert_send(&Call::<F, _>::new(ReadFramedChunks::new()));
    }
    build::<Direct<&[u8]>>();
}

/// `yield` in closures and nested items keeps its own meaning.
#[call(yields = u8)]
async fn yield_outside_closures(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
) -> u8 {
    fn helper() -> u8 {
        7
    }
    let [byte] = io.read_array::<1>().await.unwrap();
    let shift = |value: u8| value + helper();
    yield shift(byte);
    byte
}

#[test]
fn only_yields_of_the_function_body_are_items() {
    let mut input: &'static [u8] = b"\x01";
    let mut call = pin!(Call::new(YieldOutsideClosures::new()));
    assert_eq!(events(call.as_mut(), &mut input), (vec![8], 1));
}

/// A capability trait with a type parameter.
#[capability]
pub trait Fill<T: Copy>: ReceiverFamily {
    fn poll_fill<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut [T],
    ) -> Poll<usize>
    where
        Self: 'visit;
}
/// Fills with its value, as bytes or as words.
struct Pattern(u8);
direct_receiver!(impl [] for Pattern);
impl Fill<u8> for Direct<Pattern> {
    fn poll_fill<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        destination: &mut [u8],
    ) -> Poll<usize>
    where
        Self: 'visit,
    {
        destination.fill(host.0.0);
        Poll::Ready(destination.len())
    }
}
impl Fill<u16> for Direct<Pattern> {
    fn poll_fill<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        destination: &mut [u16],
    ) -> Poll<usize>
    where
        Self: 'visit,
    {
        destination.fill(u16::from_le_bytes([host.0.0; 2]));
        Poll::Ready(destination.len())
    }
}

#[call(sync)]
async fn fill_both(
    io: &mut impl ReceiverAccess<Family: Fill<u8> + Fill<u16>>,
) -> ([u8; 2], [u16; 1]) {
    let (mut bytes, mut words) = ([0; 2], [0; 1]);
    // The argument selects the trait's parameter.
    io.fill(&mut bytes).await;
    io.fill(&mut words).await;
    (bytes, words)
}

#[test]
fn a_generic_capability_generates_generic_operations() {
    let mut pattern = Pattern(1);
    assert_eq!(pattern.sync_fill_both_unpin(), ([1, 1], [0x0101]));
    let mut words = [0_u16; 3];
    let _: fill::Fill<'_, u16> = fill::Fill::new(&mut words);
    assert_eq!(Pattern(2).fill_unpin(&mut words[..]).poll_ready_now(), 3);
    assert_eq!(words, [0x0202; 3]);
}

trait PollReadyNow: std::future::Future + Sized {
    fn poll_ready_now(self) -> Self::Output {
        match pin!(self).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("future was not ready"),
        }
    }
}
impl<T: std::future::Future> PollReadyNow for T {}

/// A family predicate naming only the traits' parameters.
#[call]
async fn fill_pair<T: Copy + Default, A>(io: &mut A) -> [T; 2]
where
    A: ReceiverAccess<Family: Fill<T>>,
    A::Family: Fill<T>,
{
    let mut pair = [T::default(); 2];
    io.fill(&mut pair).await;
    pair
}

#[test]
fn family_predicates_apply_to_the_family_in_conveniences() {
    let pair: [u8; 2] = Pattern(3).fill_pair_unpin().poll_ready_now();
    assert_eq!(pair, [3; 2]);
}

/// A family predicate naming no parameters at all.
#[call]
async fn clear_then_write<A>(io: &mut A, source: &[u8]) -> usize
where
    A: ReceiverAccess<Family: Write>,
    A::Family: Clear,
{
    io.clear_all().await;
    io.write(source).await
}

#[test]
fn family_predicates_without_parameters_apply_to_the_family() {
    let mut sink = b"old".to_vec();
    assert_eq!(sink.clear_then_write_unpin(b"x").poll_ready_now(), 1);
    assert_eq!(sink, b"x");
}

#[test]
fn a_one_off_operation_is_a_local_function() {
    // Generated items stay local to the block, as a closure would.
    #[call]
    async fn first_byte(io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>) -> u8 {
        let [byte] = io.read_array::<1>().await.unwrap();
        byte
    }
    let mut input = b"q".as_slice();
    let mut call = pin!(Call::<Direct<&[u8]>, _>::new(FirstByte::new()));
    let result = call.as_mut().poll_with(
        Some(pin!(Pin::new(&mut input).view())),
        &mut Context::from_waker(Waker::noop()),
    );
    assert!(matches!(result, CallPoll::Ready(ControlFlow::Break(b'q'))));
}

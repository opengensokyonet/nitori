#![feature(impl_trait_in_assoc_type)]
use bytes::BufMut;
use nitori_call_prototype::{
    BorrowedScope, Call, Compose, Direct, HasReceiverFamily, ReceiverAccess, ReceiverFamily,
    ReceiverSlot, Start, ViewLoan, call, capability, direct_receiver,
};
use std::{
    collections::VecDeque,
    convert::Infallible,
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
    let mut call = pin!(Call::new(read::Read::new(&mut destination)));
    let result = call.as_mut().poll_with(
        pin!(BorrowedScope::new(Pin::new(&mut input))),
        &mut Context::from_waker(Waker::noop()),
    );
    assert!(matches!(result, Poll::Ready(ControlFlow::Break(Ok(2)))));
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
impl<F: ReceiverFamily> HasReceiverFamily for LimitedView<'_, F> {
    type Family = Limited<F>;
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
    let mut call = pin!(Call::new(rewriting::Rewrite::new(b"new")));
    let result = call.as_mut().poll_with(
        pin!(BorrowedScope::new(Pin::new(&mut sink))),
        &mut Context::from_waker(Waker::noop()),
    );
    assert!(matches!(result, Poll::Ready(ControlFlow::Break(3))));
    assert_eq!(sink, b"new");
    let _: writing::Write<'_> = writing::Write::new(b"");
    let _: ClearAll = ClearAll::new();
}

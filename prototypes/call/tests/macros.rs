use bytes::BufMut;
use nitori_call_prototype::{
    Call, Compose, Direct, Emitter, HasReceiverFamily, ReceiverAccess, ReceiverFamily,
    ReceiverSlot, ViewLoan, call, capability, direct_receiver,
};
use std::{
    collections::VecDeque,
    convert::Infallible,
    marker::PhantomData,
    pin::{Pin, pin},
    task::{Context, Poll, Waker, ready},
};

/// The same contract as `nitori_io::Read`; the macro adds its operations.
#[capability]
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

#[call]
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

#[call]
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

#[call]
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

#[call]
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

async fn record_root<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    _: Emitter<'_, Infallible>,
    output: &mut Vec<u8>,
) -> Result<usize, Incomplete> {
    io.read_record(output).await
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn a_call_using_generated_methods_over_a_generic_family_is_send() {
    fn build<F: Read<Error = Infallible> + 'static>(output: &mut Vec<u8>) {
        let call = Call::new((record_root::<F>, output));
        assert_send(&call);
    }
    build::<Direct<&'static [u8]>>(&mut Vec::new());
}

//! A byte capability, memory and waiting resources, and hand-written operations.
#![allow(dead_code)]
use bytes::BufMut;
use nitori_call::{Compose, Direct, ReceiverAccess, ReceiverFamily, ViewLoan, direct_receiver};
use std::{
    collections::VecDeque,
    convert::Infallible,
    future::{Future, poll_fn},
    marker::PhantomData,
    pin::{Pin, pin},
    task::{Context, Poll, Waker, ready},
};

/// The same contract as `nitori_io::Read`, declared over this crate's families.
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
        let source = host.get_pin_mut().get_mut();
        let count = source.len().min(destination.remaining_mut());
        destination.put_slice(&source[..count]);
        *source = &source[count..];
        Poll::Ready(Ok(count))
    }
}

/// Bytes that arrive over time; reading an open, empty feed waits.
#[derive(Default)]
pub struct Feed {
    pub data: VecDeque<u8>,
    pub closed: bool,
    pub waker: Option<Waker>,
}
impl Feed {
    pub fn new(data: &[u8]) -> Self {
        Self {
            data: data.iter().copied().collect(),
            ..Self::default()
        }
    }
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
        let feed = host.get_pin_mut().get_mut();
        if feed.data.is_empty() && !feed.closed && destination.has_remaining_mut() {
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
pub enum ReadError<E> {
    Incomplete { completed: usize },
    Source(E),
}

/// Read once into `destination`.
pub fn read<'a, A, O>(
    io: &'a mut A,
    destination: &'a mut O,
) -> impl Future<Output = Result<usize, <A::Family as Read>::Error>> + 'a
where
    A: ReceiverAccess,
    A::Family: Read,
    O: BufMut + ?Sized,
{
    poll_fn(move |cx| {
        io.poll_access(cx, |access, cx| {
            <A::Family as Read>::poll_read(access.into_pin(), cx, destination)
        })
    })
}

/// Read exactly `LENGTH` bytes.
pub async fn read_array<const LENGTH: usize, A>(
    io: &mut A,
) -> Result<[u8; LENGTH], ReadError<<A::Family as Read>::Error>>
where
    A: ReceiverAccess,
    A::Family: Read,
{
    let mut array = [0; LENGTH];
    let mut filled = 0;
    while filled < LENGTH {
        match read(io, &mut &mut array[filled..]).await {
            Ok(0) => return Err(ReadError::Incomplete { completed: filled }),
            Ok(count) => filled += count,
            Err(error) => return Err(ReadError::Source(error)),
        }
    }
    Ok(array)
}

/// Append exactly `length` bytes to `output`.
pub async fn read_exact<A>(
    io: &mut A,
    output: &mut Vec<u8>,
    length: usize,
) -> Result<(), ReadError<<A::Family as Read>::Error>>
where
    A: ReceiverAccess,
    A::Family: Read,
{
    let mut completed = 0;
    while completed < length {
        let mut limited = output.limit(length - completed);
        match read(io, &mut limited).await {
            Ok(0) => return Err(ReadError::Incomplete { completed }),
            Ok(count) => completed += count,
            Err(error) => return Err(ReadError::Source(error)),
        }
    }
    Ok(())
}

/// A byte budget owned by the operation, retained across polls.
pub struct ReadLimit(pub usize);
pub struct Limited<F>(PhantomData<fn(F) -> F>);
pub struct LimitedView<'v, F: ReceiverFamily> {
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

/// Poll once with a no-op waker.
pub fn poll_once<T>(future: Pin<&mut impl Future<Output = T>>) -> Poll<T> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Drive a future that must not wait.
pub fn run_ready<T>(future: impl Future<Output = T>) -> T {
    match poll_once(pin!(future)) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("future was not ready"),
    }
}

/// Counts the wakes it receives.
#[derive(Default)]
pub struct WakeCount(pub std::sync::atomic::AtomicUsize);
impl std::task::Wake for WakeCount {
    fn wake(self: std::sync::Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
impl WakeCount {
    pub fn new() -> (std::sync::Arc<Self>, Waker) {
        let count = std::sync::Arc::new(Self::default());
        let waker = Waker::from(count.clone());
        (count, waker)
    }
    pub fn get(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Polls its branch with a waker of its own, and only after that waker
/// fired, as executors such as `FuturesUnordered` do.
pub async fn only_when_woken<T>(branch: impl Future<Output = T>) -> T {
    let (wakes, waker) = WakeCount::new();
    let mut branch = pin!(branch);
    let mut seen = None;
    poll_fn(|_| {
        if seen == Some(wakes.get()) {
            return Poll::Pending;
        }
        seen = Some(wakes.get());
        branch.as_mut().poll(&mut Context::from_waker(&waker))
    })
    .await
}

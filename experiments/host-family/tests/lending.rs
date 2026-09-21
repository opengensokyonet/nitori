#![feature(coroutines, coroutine_trait)]

use host_family::lending::{Composed, Layer, Layered, ReadLayer, Reborrow, Root, with};
use host_family::{Call, Family, Read, ReadFamily, Resume, Suspend, decode, stack};
use std::{
    cell::{Cell, RefCell},
    marker::{PhantomData, PhantomPinned},
    ops::CoroutineState::{Complete, Yielded},
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
};

type Reports<'data> = Rc<RefCell<Vec<(&'data str, usize)>>>;
struct Counter<'data> {
    label: &'data str,
    reads: Cell<usize>,
    reports: Reports<'data>,
    _pin: PhantomPinned,
}
impl<'data> Counter<'data> {
    fn new(label: &'data str, reports: Reports<'data>) -> Self {
        Self {
            label,
            reads: Cell::new(0),
            reports,
            _pin: PhantomPinned,
        }
    }
}
impl Drop for Counter<'_> {
    fn drop(&mut self) {
        self.reports
            .borrow_mut()
            .push((self.label, self.reads.get()));
    }
}

struct View<'visit, 'data, F: Family + 'visit> {
    inner: F::Host<'visit>,
    counter: Pin<&'visit mut Counter<'data>>,
    // No lifetime subtyping can perform this reconstruction, even though the
    // author can safely rebuild the view from freshly borrowed components.
    invariant: PhantomData<fn(&'visit ()) -> &'visit ()>,
    _pin: PhantomPinned,
}

impl<'data, F: Reborrow> Layer<F> for Counter<'data> {
    type Host<'visit>
        = View<'visit, 'data, F>
    where
        Self: 'visit,
        F: 'visit;
    fn compose<'visit>(self: Pin<&'visit mut Self>, inner: F::Host<'visit>) -> Self::Host<'visit>
    where
        F: 'visit,
    {
        View {
            inner,
            counter: self,
            invariant: PhantomData,
            _pin: PhantomPinned,
        }
    }
    fn reborrow<'short, 'host>(host: Pin<&'short mut Self::Host<'host>>) -> Self::Host<'short>
    where
        Self: 'host,
        F: 'host,
        'host: 'short,
    {
        // SAFETY: inner is structurally pinned and only accessed via Pin.
        // counter is a pinned pointer; reconstructing it never moves Counter.
        let old = unsafe { host.get_unchecked_mut() };
        let inner = F::reborrow(unsafe { Pin::new_unchecked(&mut old.inner) });
        View {
            inner,
            counter: old.counter.as_mut(),
            invariant: PhantomData,
            _pin: PhantomPinned,
        }
    }
}

impl<F: ReadFamily> Read for View<'_, '_, F> {
    type Error = F::Error;
    fn read(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u8, Self::Error>> {
        // SAFETY: inner is structurally pinned and is never moved or exposed
        // unpinned; the counter field remains a pinned reference to its owner.
        let this = unsafe { self.get_unchecked_mut() };
        this.counter.reads.set(this.counter.reads.get() + 1);
        F::read(unsafe { Pin::new_unchecked(&mut this.inner) }, cx)
    }
}

trait Tagged<'data> {
    fn stats(&self) -> (&'data str, usize);
}
impl<'data, F: Family> Tagged<'data> for View<'_, 'data, F> {
    fn stats(&self) -> (&'data str, usize) {
        (self.counter.label, self.counter.reads.get())
    }
}

impl<F: ReadFamily + Reborrow> ReadLayer<F> for Counter<'_> {
    type Error = F::Error;
    fn read<'host>(
        host: Pin<&mut Self::Host<'host>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<u8, F::Error>>
    where
        Self: 'host,
        F: 'host,
    {
        host.read(cx)
    }
}

struct Input<'data> {
    bytes: &'data [u8],
    eof: &'data str,
    position: Cell<usize>,
    waiting: Cell<bool>,
    _pin: PhantomPinned,
}
impl<'data> Read for Input<'data> {
    type Error = &'data str;
    fn read(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u8, Self::Error>> {
        if self.waiting.replace(false) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        match self.bytes.get(self.position.get()) {
            Some(byte) => {
                self.position.set(self.position.get() + 1);
                self.waiting.set(true);
                Poll::Ready(Ok(*byte))
            }
            None => Poll::Ready(Err(self.eof)),
        }
    }
}

fn inspected_decode<'data, F: ReadFamily + Reborrow>(
    label: &'data str,
) -> impl Call<Layered<F, Counter<'data>>, Yield = u8, Return = Result<[u8; 2], F::Error>> {
    // SAFETY: environments are used only synchronously in the current resume,
    // consumed before yielding, and never escape into output or Drop.
    unsafe {
        stack(
            #[coroutine]
            static move |mut env: Resume<Layered<F, Counter<'data>>>| {
                // The closure captures a non-static mutable local. Its output may
                // retain the original data borrow, but not the temporary host borrow.
                let mut observed = 0;
                let (saved_label, first_count) = {
                    let mut request = pin!(with::<Layered<F, Counter<'data>>, _, _>(|access| {
                        observed += 1;
                        let host = access.into_pin();
                        host.stats()
                    }));
                    match env.poll(request.as_mut()) {
                        Poll::Ready(Complete(value)) => value,
                        _ => unreachable!(),
                    }
                };
                assert_eq!(observed, 1);
                assert_eq!(saved_label, label);
                assert_eq!(first_count, 0);
                let mut child = pin!(decode::<Layered<F, Counter<'data>>>());
                loop {
                    match env.poll(child.as_mut()) {
                        Poll::Pending => {
                            env.end();
                            env = yield Suspend::Pending;
                        }
                        Poll::Ready(Yielded(value)) => {
                            env.end();
                            env = yield Suspend::Yield(value);
                        }
                        Poll::Ready(Complete(value)) => {
                            // saved_label survived all suspensions without becoming static.
                            assert_eq!(saved_label, label);
                            env.end();
                            break value;
                        }
                    }
                }
            },
        )
    }
}

fn inner<'data, F: ReadFamily + Reborrow>(
    label: &'data str,
    reports: Reports<'data>,
) -> impl Call<F, Yield = u8, Return = Result<[u8; 2], F::Error>> {
    // SAFETY: current-resume use only, with explicit end before suspension.
    unsafe {
        stack(
            #[coroutine]
            static move |mut env: Resume<F>| {
                // This state does not exist when the outer call constructs its view.
                let mut state = pin!(Counter::new(label, reports));
                let mut child = pin!(Composed::<F, _, _>::new(
                    state.as_mut(),
                    inspected_decode::<F>(label)
                ));
                loop {
                    match env.poll(child.as_mut()) {
                        Poll::Pending => {
                            env.end();
                            env = yield Suspend::Pending;
                        }
                        Poll::Ready(Yielded(value)) => {
                            env.end();
                            env = yield Suspend::Yield(value);
                        }
                        Poll::Ready(Complete(value)) => {
                            env.end();
                            break value;
                        }
                    }
                }
            },
        )
    }
}

fn outer<'data, F: ReadFamily + Reborrow>(
    labels: [&'data str; 2],
    reports: Reports<'data>,
) -> impl Call<F, Yield = u8, Return = Result<[u8; 2], F::Error>> {
    // SAFETY: current-resume use only, with explicit end before suspension.
    unsafe {
        stack(
            #[coroutine]
            static move |mut env: Resume<F>| {
                let mut state = pin!(Counter::new(labels[0], reports.clone()));
                let operation = inner::<Layered<F, Counter<'data>>>(labels[1], reports);
                let mut child = pin!(Composed::<F, _, _>::new(state.as_mut(), operation));
                loop {
                    match env.poll(child.as_mut()) {
                        Poll::Pending => {
                            env.end();
                            env = yield Suspend::Pending;
                        }
                        Poll::Ready(Yielded(value)) => {
                            env.end();
                            env = yield Suspend::Yield(value);
                        }
                        Poll::Ready(Complete(value)) => {
                            env.end();
                            break value;
                        }
                    }
                }
            },
        )
    }
}

#[test]
fn child_adds_local_layer_on_an_existing_invariant_family_host() {
    let data = vec![4, 7];
    let eof = String::from("eof");
    let labels = [String::from("outer"), String::from("inner")];
    let reports = Rc::new(RefCell::new(Vec::new()));
    let mut host = pin!(Input {
        bytes: &data,
        eof: &eof,
        position: Cell::new(0),
        waiting: Cell::new(true),
        _pin: PhantomPinned
    });
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut call = pin!(outer::<Root<Input<'_>>>(
            [&labels[0], &labels[1]],
            reports.clone()
        ));
        for event in [
            Poll::Pending,
            Poll::Ready(Yielded(4)),
            Poll::Pending,
            Poll::Ready(Yielded(7)),
            Poll::Ready(Complete(Ok([4, 7]))),
        ] {
            let mut view = host.as_mut();
            assert_eq!(call.as_mut().poll(Pin::new(&mut view), &mut cx), event);
        }
    }
    assert_eq!(
        &*reports.borrow(),
        &[(labels[1].as_str(), 4), (labels[0].as_str(), 4)]
    );
}

#[test]
fn cancellation_releases_nested_views_without_resetting_underlying_host() {
    let data = vec![2, 5];
    let eof = String::from("eof");
    let labels = [String::from("outer"), String::from("inner")];
    let reports = Rc::new(RefCell::new(Vec::new()));
    let mut host = pin!(Input {
        bytes: &data,
        eof: &eof,
        position: Cell::new(0),
        waiting: Cell::new(true),
        _pin: PhantomPinned
    });
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut call = pin!(outer::<Root<Input<'_>>>(
            [&labels[0], &labels[1]],
            reports.clone()
        ));
        for event in [Poll::Pending, Poll::Ready(Yielded(2))] {
            let mut view = host.as_mut();
            assert_eq!(call.as_mut().poll(Pin::new(&mut view), &mut cx), event);
        }
    }
    assert_eq!(
        &*reports.borrow(),
        &[(labels[1].as_str(), 2), (labels[0].as_str(), 2)]
    );
    assert_eq!(host.position.get(), 1);
    assert_eq!(host.as_mut().read(&mut cx), Poll::Pending);
    assert_eq!(host.as_mut().read(&mut cx), Poll::Ready(Ok(5)));
}

#[test]
fn nominal_with_accepts_non_static_root_host_and_borrowed_output() {
    let text = String::from("borrowed");
    let mut host = text.as_str();
    let mut view = Pin::new(&mut host);
    let mut cx = Context::from_waker(Waker::noop());
    let mut call = pin!(with::<Root<&str>, _, _>(|access| **access.into_pin()));
    assert_eq!(
        call.as_mut().poll(Pin::new(&mut view), &mut cx),
        Poll::Ready(Complete(text.as_str()))
    );
}

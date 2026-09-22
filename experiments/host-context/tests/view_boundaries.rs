#![feature(coroutines, stmt_expr_attributes)]
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::multiple_bound_locations)] // pin-project-lite projections
//! Optional canonical access at a coroutine boundary. Stored views keep their
//! exact types; only newly created access values have a common scope lifetime.

use host_context::{
    AwaitOn, CallOn, HasFamily, HostContext, HostFamily,
    coroutine::{self, ResumeEnv, Suspend},
};
use std::{
    cell::Cell,
    marker::{PhantomData, PhantomPinned},
    pin::{Pin, pin},
    sync::{Mutex, MutexGuard},
    task::{Context, Poll, Waker, ready},
};

trait CanonicalAccess: HasFamily {
    fn access<'scope>(
        self: Pin<&'scope mut Self>,
    ) -> <Self::Family as HostFamily>::HostView<'scope>
    where
        Self::Family: 'scope;
}

// The parent lends its exact stored type, independently of any Family mapping.
// Ready must remain ready and lend the same pinned value until context drop.
trait ActualContext {
    type View;
    fn poll_view(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Pin<&mut Self::View>>;
}
pin_project_lite::pin_project! {
    struct Lazy<M, V> { make: Option<M>, #[pin] view: Option<V> }
}
impl<M: FnOnce() -> V, V> ActualContext for Lazy<M, V> {
    type View = V;
    fn poll_view(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Pin<&mut V>> {
        let mut this = self.project();
        if this.view.is_none() {
            this.view.set(Some(this.make.take().unwrap()()));
        }
        Poll::Ready(this.view.as_pin_mut().unwrap())
    }
}
struct Borrowed<'a, V> {
    view: Pin<&'a mut V>,
}
impl<V> ActualContext for Borrowed<'_, V> {
    type View = V;
    fn poll_view(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Pin<&mut V>> {
        Poll::Ready(self.get_mut().view.as_mut())
    }
}
pin_project_lite::pin_project! {
    struct CanonicalContext<'scope, V: CanonicalAccess>
    where V::Family: 'scope,
    {
        parent: Option<Pin<&'scope mut dyn ActualContext<View = V>>>,
        #[pin]
        access: Option<<V::Family as HostFamily>::HostView<'scope>>,
    }
}
impl<'scope, V: CanonicalAccess> HostContext<'scope> for CanonicalContext<'scope, V>
where
    V::Family: 'scope,
{
    type Family = V::Family;
    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut this = self.project();
        if this.access.is_none() {
            ready!(this.parent.as_mut().unwrap().as_mut().poll_view(cx));
            let Poll::Ready(view) = this.parent.take().unwrap().poll_view(cx) else {
                panic!("parent readiness must remain ready");
            };
            this.access.set(Some(view.access()));
        }
        Poll::Ready(())
    }
    fn ready_view<'a>(
        self: Pin<&'a mut Self>,
    ) -> Pin<&'a mut <Self::Family as HostFamily>::HostView<'scope>> {
        self.project().access.as_pin_mut().unwrap()
    }
}

// Reuse the library's capability-independent ResumeEnv and request dispatch.
// This coroutine awaits one external wakeup before touching its operation.
fn wait_then_run<F: HostFamily, A: AwaitOn<F>>(
    operation: A,
) -> impl CallOn<F, Yield = (), Return = A::Output> {
    let state = #[coroutine]
    static move |mut env: ResumeEnv<F>| {
        let mut pending = true;
        let input = std::future::poll_fn(move |cx| {
            if std::mem::take(&mut pending) {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        });
        let mut input = pin!(input);
        // SAFETY: only the current resume's token is accessed, and it is
        // consumed before every suspension or completion.
        while unsafe { env.poll_await(input.as_mut()) }.is_pending() {
            env.end();
            env = yield Suspend::Pending;
        }
        let mut operation = pin!(operation);
        let result = loop {
            // SAFETY: the same current-resume protocol applies to the child.
            match unsafe { env.poll_await(operation.as_mut()) } {
                Poll::Pending => {
                    env.end();
                    env = yield Suspend::Pending;
                }
                Poll::Ready(value) => break value,
            }
        };
        env.end();
        result
    };
    // SAFETY: neither await retains a token; every yield and return consumes it.
    unsafe { coroutine::build(state) }
}

// Static normalization: expensive storage stays in the owning root. A canonical
// access contains only fresh field borrows, never a second guard or parsed value.
struct Data<'data> {
    name: &'data str,
    value: u8,
}
struct Root<'data>(PhantomData<&'data str>);
struct RootAccess<'a, 'data> {
    data: &'a mut Data<'data>,
    parsed: &'a str,
}
impl<'data> HostFamily for Root<'data> {
    type HostView<'a>
        = RootAccess<'a, 'data>
    where
        Self: 'a;
}
impl<'data> HasFamily for RootAccess<'_, 'data> {
    type Family = Root<'data>;
}
pin_project_lite::pin_project! {
    struct Owned<'lock, 'data> {
        guard: MutexGuard<'lock, Data<'data>>,
        parsed: String,
        access_count: &'data Cell<usize>,
        invariant: PhantomData<fn(&'lock ()) -> &'lock ()>,
        #[pin]
        pinned: PhantomPinned,
    }
}
impl<'data> HasFamily for Owned<'_, 'data> {
    type Family = Root<'data>;
}
impl<'data> CanonicalAccess for Owned<'_, 'data> {
    fn access<'a>(self: Pin<&'a mut Self>) -> RootAccess<'a, 'data>
    where
        Root<'data>: 'a,
    {
        let this = self.project();
        this.access_count.set(this.access_count.get() + 1);
        RootAccess {
            data: &mut *this.guard,
            parsed: this.parsed,
        }
    }
}
struct Counted<F>(PhantomData<fn() -> F>);
pin_project_lite::pin_project! {
    struct CountAccess<'a, F: HostFamily> where F: 'a {
        #[pin]
        inner: F::HostView<'a>,
        count: &'a mut usize,
    }
}
impl<F: HostFamily> HostFamily for Counted<F> {
    type HostView<'a>
        = CountAccess<'a, F>
    where
        Self: 'a;
}
impl<F: HostFamily> HasFamily for CountAccess<'_, F> {
    type Family = Counted<F>;
}
struct Layer<'a, V> {
    inner: Pin<&'a mut V>,
    count: &'a mut usize,
    access_count: &'a Cell<usize>,
}
impl<V: HasFamily> HasFamily for Layer<'_, V> {
    type Family = Counted<V::Family>;
}
impl<V: CanonicalAccess> CanonicalAccess for Layer<'_, V> {
    fn access<'a>(self: Pin<&'a mut Self>) -> CountAccess<'a, V::Family>
    where
        V::Family: 'a,
    {
        let this = self.get_mut();
        this.access_count.set(this.access_count.get() + 1);
        CountAccess {
            inner: this.inner.as_mut().access(),
            count: this.count,
        }
    }
}
trait Read: HostFamily {
    fn read<'v>(view: Pin<&mut Self::HostView<'v>>) -> u8
    where
        Self: 'v;
}
impl Read for Root<'_> {
    fn read<'v>(view: Pin<&mut Self::HostView<'v>>) -> u8
    where
        Self: 'v,
    {
        let view = view.get_mut();
        assert_eq!(view.parsed, view.data.name.to_uppercase());
        let byte = view.data.value;
        view.data.value += 1;
        byte
    }
}
impl<F: Read> Read for Counted<F> {
    fn read<'v>(view: Pin<&mut Self::HostView<'v>>) -> u8
    where
        Self: 'v,
    {
        let this = view.project();
        **this.count += 1;
        F::read(this.inner)
    }
}
struct ReadTwice;
impl<F: Read> AwaitOn<F> for ReadTwice {
    type Output = (u8, u8);
    fn poll_on<'v>(
        self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'v, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        F: 'v,
    {
        let mut view = ready!(context.as_mut().poll_view(cx));
        Poll::Ready((F::read(view.as_mut()), F::read(view)))
    }
}

#[test]
fn normalize_recursive_views_only_at_the_coroutine_boundary() {
    let label = String::from("borrowed");
    let access_counts = [Cell::new(0), Cell::new(0), Cell::new(0)];
    let mutex = Mutex::new(Data {
        name: &label,
        value: 7,
    });
    let mut counts = [0, 0];
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut root = pin!(Owned {
            guard: mutex.lock().unwrap(),
            parsed: label.to_uppercase(),
            access_count: &access_counts[0],
            invariant: PhantomData,
            pinned: PhantomPinned
        });
        let [count1, count2] = &mut counts;
        let mut layer1 = pin!(Layer {
            inner: root.as_mut(),
            count: count1,
            access_count: &access_counts[1]
        });
        let mut layer2 = pin!(Layer {
            inner: layer1.as_mut(),
            count: count2,
            access_count: &access_counts[2]
        });
        let mut parent = pin!(Borrowed {
            view: layer2.as_mut()
        });
        let mut call = pin!(wait_then_run::<Counted<Counted<Root<'_>>>, _>(ReadTwice));
        {
            let mut boundary = pin!(CanonicalContext {
                parent: Some(parent.as_mut()),
                access: None
            });
            assert_eq!(
                call.as_mut().poll_return(boundary.as_mut(), &mut cx),
                Poll::Pending
            );
            assert_eq!(access_counts.each_ref().map(Cell::get), [0, 0, 0]);
        }
        {
            let mut boundary = pin!(CanonicalContext {
                parent: Some(parent.as_mut()),
                access: None
            });
            assert_eq!(
                call.as_mut().poll_return(boundary.as_mut(), &mut cx),
                Poll::Ready((7, 8))
            );
            assert_eq!(access_counts.each_ref().map(Cell::get), [1, 1, 1]);
        }
        assert!(mutex.try_lock().is_err()); // root still owns the original guard
    }
    assert_eq!(counts, [2, 2]);
    assert_eq!(mutex.try_lock().unwrap().value, 9);
}

// Generic erasure infrastructure: the schema chooses an interface, and the
// execution machinery has no knowledge of its capabilities or output types.
trait Schema {
    type Interface<'a>: ?Sized
    where
        Self: 'a;
}
struct Erased<S>(PhantomData<fn() -> S>);
struct ErasedAccess<'a, S: Schema + 'a> {
    interface: Pin<&'a mut S::Interface<'a>>,
}
impl<S: Schema> HostFamily for Erased<S> {
    type HostView<'a>
        = ErasedAccess<'a, S>
    where
        Self: 'a;
}
impl<S: Schema> HasFamily for ErasedAccess<'_, S> {
    type Family = Erased<S>;
}
trait Erase<S: Schema> {
    fn erase<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut S::Interface<'a>>
    where
        S: 'a;
}
pin_project_lite::pin_project! {
    struct EraseAs<S, V> { #[pin] inner: V, schema: PhantomData<fn() -> S> }
}
impl<S: Schema, V> HasFamily for EraseAs<S, V> {
    type Family = Erased<S>;
}
impl<S: Schema, V: Erase<S>> CanonicalAccess for EraseAs<S, V> {
    fn access<'a>(self: Pin<&'a mut Self>) -> ErasedAccess<'a, S>
    where
        S: 'a,
    {
        ErasedAccess {
            interface: self.project().inner.erase(),
        }
    }
}

// An application-defined set of two capabilities, including a borrowed return.
trait Sequence {
    fn next(self: Pin<&mut Self>) -> u8;
}
trait Label {
    fn label(&self) -> &str;
}
trait Device: Sequence + Label {}
impl<V: Sequence + Label> Device for V {}
struct DeviceSchema<'data>(PhantomData<&'data str>);
impl Schema for DeviceSchema<'_> {
    type Interface<'a>
        = dyn Device + 'a
    where
        Self: 'a;
}
impl<'data, V: Device> Erase<DeviceSchema<'data>> for V {
    fn erase<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut (dyn Device + 'a)>
    where
        DeviceSchema<'data>: 'a,
    {
        self
    }
}
// Operation logic can remain on logical families; these object-safe methods
// are blanket forwarding facades, not framework-defined IO operations.
trait SequenceAt<V: ?Sized>: HostFamily {
    fn next(view: Pin<&mut V>) -> u8;
}
trait LabelAt<V: ?Sized>: HostFamily {
    fn label(view: &V) -> &str;
}
impl<V: HasFamily> Sequence for V
where
    V::Family: SequenceAt<V>,
{
    fn next(self: Pin<&mut Self>) -> u8 {
        V::Family::next(self)
    }
}
impl<V: HasFamily> Label for V
where
    V::Family: LabelAt<V>,
{
    fn label(&self) -> &str {
        V::Family::label(self)
    }
}
impl<'data> SequenceAt<Owned<'_, 'data>> for Root<'data> {
    fn next(view: Pin<&mut Owned<'_, 'data>>) -> u8 {
        let this = view.project();
        let byte = this.guard.value;
        this.guard.value += 1;
        byte
    }
}
impl<'data> LabelAt<Owned<'_, 'data>> for Root<'data> {
    fn label<'a>(view: &'a Owned<'_, 'data>) -> &'a str {
        &view.parsed
    }
}
impl<V: HasFamily + Sequence> SequenceAt<Layer<'_, V>> for Counted<V::Family> {
    fn next(view: Pin<&mut Layer<'_, V>>) -> u8 {
        let this = view.get_mut();
        *this.count += 1;
        this.inner.as_mut().next()
    }
}
impl<V: HasFamily + Label> LabelAt<Layer<'_, V>> for Counted<V::Family> {
    fn label<'a>(view: &'a Layer<'_, V>) -> &'a str {
        view.inner.as_ref().get_ref().label()
    }
}
struct InspectDevice;
impl<'data> AwaitOn<Erased<DeviceSchema<'data>>> for InspectDevice {
    type Output = (u8, u8);
    fn poll_on<'v>(
        self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'v, Family = Erased<DeviceSchema<'data>>>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        Erased<DeviceSchema<'data>>: 'v,
    {
        let mut view = ready!(context.as_mut().poll_view(cx));
        let interface = &mut view.as_mut().get_mut().interface;
        assert_eq!(interface.as_ref().get_ref().label(), "BORROWED");
        Poll::Ready((interface.as_mut().next(), interface.as_mut().next()))
    }
}

fn inspect_borrowed_device<'data>(
    _label: &'data str,
) -> impl CallOn<Erased<DeviceSchema<'data>>, Yield = (), Return = (u8, u8)> {
    wait_then_run(InspectDevice)
}

#[test]
fn erase_recursive_view_through_an_application_defined_capability_set() {
    let label = String::from("borrowed");
    let access_counts = [Cell::new(0), Cell::new(0), Cell::new(0)];
    let mutex = Mutex::new(Data {
        name: &label,
        value: 7,
    });
    let mut counts = [0, 0];
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut root = pin!(Owned {
            guard: mutex.lock().unwrap(),
            parsed: label.to_uppercase(),
            access_count: &access_counts[0],
            invariant: PhantomData,
            pinned: PhantomPinned
        });
        let [count1, count2] = &mut counts;
        let mut layer1 = pin!(Layer {
            inner: root.as_mut(),
            count: count1,
            access_count: &access_counts[1]
        });
        let layer2 = Layer {
            inner: layer1.as_mut(),
            count: count2,
            access_count: &access_counts[2],
        };
        let mut parent = pin!(Lazy {
            make: Some(|| EraseAs::<DeviceSchema<'_>, _> {
                inner: layer2,
                schema: PhantomData
            }),
            view: None
        });
        let mut call = pin!(inspect_borrowed_device(&label));
        for expected in [Poll::Pending, Poll::Ready((7, 8))] {
            let mut boundary = pin!(CanonicalContext {
                parent: Some(parent.as_mut()),
                access: None
            });
            assert_eq!(
                call.as_mut().poll_return(boundary.as_mut(), &mut cx),
                expected
            );
        }
        // Erasure borrowed the actual recursive value; no normalization occurred.
        assert_eq!(access_counts.each_ref().map(Cell::get), [0, 0, 0]);
    }
    assert_eq!(counts, [2, 2]);
    assert_eq!(mutex.try_lock().unwrap().value, 9);
}

// A completely different schema and result type uses the same context and
// ResumeEnv implementation. No sequence/label operation is in this interface.
trait Toggle {
    fn toggle(self: Pin<&mut Self>) -> bool;
}
impl Toggle for bool {
    fn toggle(self: Pin<&mut Self>) -> bool {
        let value = self.get_mut();
        *value = !*value;
        *value
    }
}
struct ToggleSchema;
impl Schema for ToggleSchema {
    type Interface<'a> = dyn Toggle + 'a;
}
impl<V: Toggle> Erase<ToggleSchema> for V {
    fn erase<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut (dyn Toggle + 'a)>
    where
        ToggleSchema: 'a,
    {
        self
    }
}
struct Flip;
impl AwaitOn<Erased<ToggleSchema>> for Flip {
    type Output = bool;
    fn poll_on<'v>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'v, Family = Erased<ToggleSchema>>>,
        cx: &mut Context<'_>,
    ) -> Poll<bool>
    where
        Erased<ToggleSchema>: 'v,
    {
        Poll::Ready(
            ready!(context.poll_view(cx))
                .get_mut()
                .interface
                .as_mut()
                .toggle(),
        )
    }
}
#[test]
fn another_schema_uses_the_same_dispatch_and_stays_lazy() {
    let constructions = Cell::new(0);
    let mut parent = pin!(Lazy {
        make: Some(|| {
            constructions.set(constructions.get() + 1);
            EraseAs::<ToggleSchema, _> {
                inner: false,
                schema: PhantomData,
            }
        }),
        view: None
    });
    let mut call = pin!(wait_then_run::<Erased<ToggleSchema>, _>(Flip));
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut boundary = pin!(CanonicalContext {
            parent: Some(parent.as_mut()),
            access: None
        });
        assert_eq!(
            call.as_mut().poll_return(boundary.as_mut(), &mut cx),
            Poll::Pending
        );
        assert_eq!(constructions.get(), 0);
    }
    {
        let mut boundary = pin!(CanonicalContext {
            parent: Some(parent.as_mut()),
            access: None
        });
        assert_eq!(
            call.as_mut().poll_return(boundary.as_mut(), &mut cx),
            Poll::Ready(true)
        );
        assert_eq!(constructions.get(), 1);
    }
}

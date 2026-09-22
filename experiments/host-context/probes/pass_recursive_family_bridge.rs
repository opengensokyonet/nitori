use std::{
    cell::Cell,
    marker::{PhantomData, PhantomPinned},
    pin::{Pin, pin},
    sync::{Mutex, MutexGuard},
    task::{Context, Poll, Waker},
};

trait Family {}
struct Root<'data>(PhantomData<&'data str>);
impl Family for Root<'_> {}
struct Counted<F: Family>(PhantomData<fn() -> F>);
impl<F: Family> Family for Counted<F> {}

trait View {
    type Family: Family;
}
trait ReadAt<V: View + ?Sized>: Family {
    fn read(view: Pin<&mut V>) -> u8;
}
trait Read: View {
    fn read(self: Pin<&mut Self>) -> u8;
}
impl<V: View + ?Sized> Read for V
where
    V::Family: ReadAt<V>,
{
    fn read(self: Pin<&mut Self>) -> u8 {
        V::Family::read(self)
    }
}

struct Data<'data> {
    label: &'data str,
    value: Cell<u8>,
}
struct RootView<'h, 'data> {
    _label: &'data str,
    guard: MutexGuard<'h, Data<'data>>,
    invariant: PhantomData<fn(&'h ()) -> &'h ()>,
    _pinned: PhantomPinned,
}
impl<'data> View for RootView<'_, 'data> {
    type Family = Root<'data>;
}
impl<'data> ReadAt<RootView<'_, 'data>> for Root<'data> {
    fn read(view: Pin<&mut RootView<'_, 'data>>) -> u8 {
        let guard = &view.as_ref().get_ref().guard;
        assert_eq!(guard.label, view.as_ref().get_ref()._label);
        let cell = &guard.value;
        let result = cell.get();
        cell.set(result + 1);
        result
    }
}

// V recursively carries every ancestor's unmodified lifetime and pinning.
struct Layer<'a, V: Read + ?Sized> {
    inner: Pin<&'a mut V>,
    count: &'a mut usize,
}
impl<V: Read + ?Sized> View for Layer<'_, V> {
    type Family = Counted<V::Family>;
}
impl<V: Read + ?Sized> ReadAt<Layer<'_, V>> for Counted<V::Family> {
    fn read(view: Pin<&mut Layer<'_, V>>) -> u8 {
        let this = view.get_mut();
        *this.count += 1;
        this.inner.as_mut().read()
    }
}

trait HostContext {
    type View: Read + ?Sized;
    fn poll_view(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Pin<&mut Self::View>>;
}
struct ReadyContext<'a, V: Read + ?Sized> {
    view: Pin<&'a mut V>,
}
impl<V: Read + ?Sized> HostContext for ReadyContext<'_, V> {
    type View = V;
    fn poll_view(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Pin<&mut V>> {
        Poll::Ready(self.get_mut().view.as_mut())
    }
}

// A generic trait instance is dyn-compatible when its actual View is fixed.
trait CallOnView<V: Read + ?Sized> {
    fn poll_call(
        &mut self,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>;
}
struct ReadOne;
impl<V: Read + ?Sized> CallOnView<V> for ReadOne {
    fn poll_call(
        &mut self,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8> {
        Poll::Ready(std::task::ready!(context.poll_view(cx)).read())
    }
}

struct Decorate<O> {
    operation: O,
    count: usize,
}
impl<O> Decorate<O> {
    fn new(operation: O) -> Self {
        Self {
            operation,
            count: 0,
        }
    }
}
impl<V: Read + ?Sized, O> CallOnView<V> for Decorate<O>
where
    for<'a> O: CallOnView<Layer<'a, V>>,
{
    fn poll_call(
        &mut self,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8> {
        let parent = std::task::ready!(context.poll_view(cx));
        let mut view = pin!(Layer {
            inner: parent,
            count: &mut self.count
        });
        let mut context = pin!(ReadyContext {
            view: view.as_mut()
        });
        self.operation.poll_call(context.as_mut(), cx)
    }
}

// Only roots with one canonical access representation implement RootFamily.
// The reference parameter supplies the outlives relation without a constrained GAT.
// Intermediate Layer values retain their actual V and all ancestor lifetimes.
trait RootFamily: Family + for<'h> RootAt<&'h Self> {}
impl RootFamily for Root<'_> {}
trait RootAt<Input> {
    type View: Read;
}
type RootViewAt<'h, F> = <F as RootAt<&'h F>>::View;
impl<'h, 'data> RootAt<&'h Root<'data>> for Root<'data> {
    type View = RootView<'h, 'data>;
}
trait CallOn<F: RootFamily> {
    fn poll_call<'h>(
        &mut self,
        context: Pin<&mut dyn HostContext<View = RootViewAt<'h, F>>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>
    where
        F: RootAt<&'h F>;
}
impl<F: RootFamily, O> CallOn<F> for O
where
    for<'h> O: CallOnView<RootViewAt<'h, F>>,
{
    fn poll_call<'h>(
        &mut self,
        context: Pin<&mut dyn HostContext<View = RootViewAt<'h, F>>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>
    where
        F: RootAt<&'h F>,
    {
        <Self as CallOnView<RootViewAt<'h, F>>>::poll_call(self, context, cx)
    }
}

// The family parameter is non-static and the mutex's data borrows it.
// Each root view additionally requires 'data: 'h through MutexGuard's T.
fn run<'data>(label: &'data str) {
    let mutex = Mutex::new(Data {
        label,
        value: Cell::new(7),
    });
    let mut operation = Decorate::new(Decorate::new(Decorate::new(ReadOne)));
    {
        let erased: &mut dyn CallOn<Root<'data>> = &mut operation;
        for expected in 7..9 {
            let mut root = pin!(RootView {
                _label: label,
                guard: mutex.lock().unwrap(),
                invariant: PhantomData,
                _pinned: PhantomPinned,
            });
            let mut context = pin!(ReadyContext {
                view: root.as_mut()
            });
            assert_eq!(
                erased.poll_call(context.as_mut(), &mut Context::from_waker(Waker::noop())),
                Poll::Ready(expected)
            );
        }
    }
    assert_eq!(operation.count, 2);
    assert_eq!(operation.operation.count, 2);
    assert_eq!(operation.operation.operation.count, 2);
    assert_eq!(mutex.lock().unwrap().value.get(), 9);
}

fn main() {
    let label = String::from("local family data");
    run(&label);
}

//! Calls lent a view on every poll: only `.await` and `yield` end a step.
#![feature(impl_trait_in_assoc_type, yield_expr)]
use nitori_call::{
    Compose, Direct, DirectView, Emitter, LentAccess, LentAccesses, LentCall, LentSlot, Operation,
    Receiver, ReceiverAccess, ReceiverFamily, Start, ViewLoan, call, capability, direct_receiver,
};
use std::{
    cell::Cell,
    collections::VecDeque,
    convert::Infallible,
    future::{Future, pending, poll_fn},
    ops::ControlFlow,
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
    thread,
};

#[capability]
pub trait Queue: ReceiverFamily {
    fn poll_next<'v>(view: Pin<&mut Self::ReceiverView<'v>>, cx: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v;
}
#[capability]
pub trait Counter: ReceiverFamily {
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, by: u32) -> u32
    where
        Self: 'v;
}

#[derive(Default)]
pub struct Items(VecDeque<u32>);
#[derive(Default)]
pub struct Total(u32);
direct_receiver!(impl [] for Items);
direct_receiver!(impl [] for Total);
impl Queue for Direct<Items> {
    fn poll_next<'v>(view: Pin<&mut Self::ReceiverView<'v>>, _: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v,
    {
        let items = view.get_pin_mut().get_mut();
        items.0.pop_front().map_or(Poll::Pending, Poll::Ready)
    }
}
impl Counter for Direct<Total> {
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, by: u32) -> u32
    where
        Self: 'v,
    {
        let total = view.get_pin_mut().get_mut();
        total.0 += by;
        total.0
    }
}

/// Only the `.await` and the `yield` give up the poll.
#[call(yields = u32)]
async fn summing<Q, C>(
    queue: &mut impl LentAccess<Family = Q>,
    counter: &mut impl LentAccess<Family = C>,
) where
    Q: Queue,
    C: Counter,
{
    loop {
        let item = queue.next().await;
        let first = counter.add(item);
        let second = counter.with(|mut view| view.add(item));
        let both = (&mut *queue, &mut *counter).with(|(_, mut view)| view.add(1));
        yield first + second + both;
    }
}

#[test]
fn a_lent_call_uses_its_views_without_awaiting() {
    let mut items = Items(VecDeque::from([2, 3]));
    let mut total = Total::default();
    let mut call = pin!(LentCall::<(Direct<Items>, Direct<Total>), _>::new(
        Summing::new()
    ));
    let mut cx = Context::from_waker(Waker::noop());
    let mut seen = Vec::new();
    for _ in 0..3 {
        let mut queue = pin!(Pin::new(&mut items).view());
        let mut counter = pin!(Pin::new(&mut total).view());
        let poll = call
            .as_mut()
            .poll_with_views((queue.as_mut(), counter.as_mut()), &mut cx);
        if let Poll::Ready(ControlFlow::Continue(item)) = poll {
            seen.push(item);
        }
    }
    assert_eq!(seen, [2 + 4 + 5, 8 + 11 + 12]);
    assert_eq!(total.0, 12);
    fn assert_send<T: Send>(_: &T) {}
    assert_send(&call);
}

/// A value that holds one component while another is used, in one step.
#[capability]
pub trait Journal: ReceiverFamily {
    type Attempt<'s>: Attempt
    where
        Self: 's;
    fn begin<'s, 'v>(view: Pin<&'s mut Self::ReceiverView<'v>>) -> Self::Attempt<'s>
    where
        Self: 'v,
        'v: 's;
}
pub trait Attempt {
    fn buffer(&mut self) -> &mut Vec<u32>;
}
#[capability]
pub trait Source: ReceiverFamily {
    fn fill<'v>(view: Pin<&mut Self::ReceiverView<'v>>, into: &mut Vec<u32>) -> usize
    where
        Self: 'v;
}

#[derive(Default)]
pub struct Log {
    packets: Vec<Vec<u32>>,
    abandoned: usize,
}
direct_receiver!(impl [] for Log);
pub struct Writing<'s> {
    log: &'s mut Log,
    frames: Vec<u32>,
}
impl Attempt for Writing<'_> {
    fn buffer(&mut self) -> &mut Vec<u32> {
        &mut self.frames
    }
}
/// The end of the attempt decides whether a packet number was used.
impl Drop for Writing<'_> {
    fn drop(&mut self) {
        if self.frames.is_empty() {
            self.log.abandoned += 1;
        } else {
            self.log.packets.push(std::mem::take(&mut self.frames));
        }
    }
}
impl Journal for Direct<Log> {
    type Attempt<'s> = Writing<'s>;
    fn begin<'s, 'v>(view: Pin<&'s mut Self::ReceiverView<'v>>) -> Writing<'s>
    where
        Self: 'v,
        'v: 's,
    {
        Writing {
            log: view.get_pin_mut().get_mut(),
            frames: Vec::new(),
        }
    }
}
impl Source for Direct<Items> {
    fn fill<'v>(view: Pin<&mut Self::ReceiverView<'v>>, into: &mut Vec<u32>) -> usize
    where
        Self: 'v,
    {
        let items = view.get_pin_mut().get_mut();
        let count = items.0.len();
        into.extend(items.0.drain(..));
        count
    }
}

#[call]
async fn filling<J, S>(
    journal: &mut impl LentAccess<Family = J>,
    source: &mut impl LentAccess<Family = S>,
    rounds: usize,
) where
    J: Journal,
    S: Source,
{
    for _ in 0..rounds {
        (&mut *journal, &mut *source).with(|(mut journal, mut source)| {
            let mut attempt = journal.begin();
            source.fill(attempt.buffer());
        });
    }
}

#[test]
fn an_attempt_spans_two_components_in_one_step() {
    let mut log = Log::default();
    let mut items = Items(VecDeque::from([7, 8]));
    let mut call = pin!(LentCall::<(Direct<Log>, Direct<Items>), _>::new(
        Filling::new(2)
    ));
    let mut cx = Context::from_waker(Waker::noop());
    let mut journal = pin!(Pin::new(&mut log).view());
    let mut source = pin!(Pin::new(&mut items).view());
    assert!(matches!(
        call.as_mut()
            .poll_with_views((journal.as_mut(), source.as_mut()), &mut cx),
        Poll::Ready(ControlFlow::Break(()))
    ));
    assert_eq!(log.packets, [vec![7, 8]]);
    assert_eq!(log.abandoned, 1);
}

type Event<T, Op> =
    ControlFlow<<Op as Operation<Direct<T>>>::Output, <Op as Operation<Direct<T>>>::Yield>;

/// One poll with a view of `value`, for operations over one family.
fn poll_on<T, Op>(call: Pin<&mut LentCall<Direct<T>, Op>>, value: &mut T) -> Poll<Event<T, Op>>
where
    T: Receiver<Family = Direct<T>> + Unpin + 'static,
    Op: Operation<Direct<T>> + for<'s> Start<'s, LentSlot<'s, Direct<T>>>,
{
    let mut view = pin!(Pin::new(value).view());
    call.poll_with_views(view.as_mut(), &mut Context::from_waker(Waker::noop()))
}

/// Operations written for a lent access have methods on it, like any other.
#[call]
async fn add_twice<F: Counter>(io: &mut impl LentAccess<Family = F>, by: u32) -> u32 {
    io.add(by);
    io.add(by)
}
#[call(yields = u32)]
async fn add_each<F: Counter, A: LentAccess<Family = F>>(io: &mut A, values: Vec<u32>) -> usize {
    let count = values.len();
    for value in values {
        yield io.add(value);
    }
    count
}
#[call(yields = u32)]
async fn by_methods(io: &mut impl LentAccess<Family: Counter>) -> u32 {
    {
        let mut child = pin!(io.add_each(vec![1, 2]));
        while let ControlFlow::Continue(item) = child.as_mut().step().await {
            yield item * 10;
        }
    }
    io.add_twice(5).await
}

#[test]
fn lent_operations_are_methods_of_lent_accesses() {
    let mut total = Total::default();
    let mut call = pin!(LentCall::<Direct<Total>, _>::new(ByMethods::new()));
    assert!(matches!(
        poll_on(call.as_mut(), &mut total),
        Poll::Ready(ControlFlow::Continue(10))
    ));
    assert!(matches!(
        poll_on(call.as_mut(), &mut total),
        Poll::Ready(ControlFlow::Continue(30))
    ));
    assert!(matches!(
        poll_on(call.as_mut(), &mut total),
        Poll::Ready(ControlFlow::Break(13))
    ));
    assert!(call.is_terminated());
}

#[test]
fn a_lent_call_can_discard_its_items() {
    let mut total = Total::default();
    let mut call = pin!(LentCall::<Direct<Total>, _>::new(AddEach::new(vec![1, 2])));
    {
        let mut view = pin!(Pin::new(&mut total).view());
        let poll = call
            .as_mut()
            .poll_return_with_views(view.as_mut(), &mut Context::from_waker(Waker::noop()));
        assert!(matches!(poll, Poll::Ready(2)));
    }
    assert_eq!(total.0, 3);
}

/// A parameter of the capability picks the implementation.
#[capability]
pub trait Fill<T>: ReceiverFamily
where
    T: Copy,
{
    type Filled;
    fn fill<'v>(view: Pin<&mut Self::ReceiverView<'v>>, value: T) -> Self::Filled
    where
        Self: 'v;
}
#[derive(Default)]
pub struct Bins(Vec<u8>, Vec<u32>);
direct_receiver!(impl [] for Bins);
impl Fill<u8> for Direct<Bins> {
    type Filled = usize;
    fn fill<'v>(view: Pin<&mut DirectView<'v, Bins>>, value: u8) -> usize
    where
        Self: 'v,
    {
        let bins = view.get_pin_mut().get_mut();
        bins.0.push(value);
        bins.0.len()
    }
}
impl Fill<u32> for Direct<Bins> {
    type Filled = u32;
    fn fill<'v>(view: Pin<&mut DirectView<'v, Bins>>, value: u32) -> u32
    where
        Self: 'v,
    {
        let bins = view.get_pin_mut().get_mut();
        bins.1.push(value);
        bins.1.iter().sum()
    }
}

#[call]
async fn filling_both<F: Fill<u8> + Fill<u32>>(
    io: &mut impl LentAccess<Family = F>,
) -> (<F as Fill<u8>>::Filled, <F as Fill<u32>>::Filled) {
    io.fill(1_u8);
    io.fill(2_u32);
    io.with(|mut view| {
        view.fill(3_u8);
        (view.fill(4_u8), view.fill(5_u32))
    })
}

#[test]
fn a_capability_parameter_picks_the_implementation() {
    let mut bins = Bins::default();
    let mut call = pin!(LentCall::<Direct<Bins>, _>::new(FillingBoth::new()));
    assert!(matches!(
        poll_on(call.as_mut(), &mut bins),
        Poll::Ready(ControlFlow::Break((3, 7)))
    ));
    assert_eq!(bins.0, [1, 3, 4]);
    assert_eq!(bins.1, [2, 5]);
}

/// Parameters of a method and lifetimes of its arguments pass through.
#[capability]
pub trait Shape: ReceiverFamily {
    /// Push every value.
    fn push_all<'v, I: IntoIterator<Item = u32>>(
        view: Pin<&mut Self::ReceiverView<'v>>,
        values: I,
    ) -> usize
    where
        Self: 'v;
    fn first_of<'v, 'x, const N: usize>(
        view: Pin<&mut <Self as ReceiverFamily>::ReceiverView<'v>>,
        values: &'x [u32; N],
    ) -> &'x u32
    where
        Self: 'v;
    fn named<'v>(view: Pin<&mut Self::ReceiverView<'v>>, name: &'static str) -> &'static str
    where
        Self: 'v;
    /// Its parameter appears in neither the view nor the arguments.
    fn default_of<'v, D: Default + std::fmt::Debug>(
        view: Pin<&mut Self::ReceiverView<'v>>,
    ) -> String
    where
        Self: 'v;
    fn converted<'v>(view: Pin<&mut Self::ReceiverView<'v>>, value: impl Into<u32>) -> u32
    where
        Self: 'v;
    fn pattern<'v>(view: Pin<&mut Self::ReceiverView<'v>>, (a, b): (u32, u32)) -> u32
    where
        Self: 'v,
    {
        let _ = view;
        a + b
    }
    /// Results borrowing the view exist only on the view.
    fn last<'s, 'v>(view: Pin<&'s mut Self::ReceiverView<'v>>) -> Option<&'s u32>
    where
        Self: 'v,
        'v: 's;
    fn marked<'v>(view: Pin<&mut Self::ReceiverView<'v>>, mark: &'v u32) -> &'v u32
    where
        Self: 'v;
    /// So do methods whose parameters must outlive the view.
    fn outliving<'v, T: 'v + Copy>(view: Pin<&mut Self::ReceiverView<'v>>, value: T) -> T
    where
        Self: 'v;
    fn outliving_too<'v, T>(view: Pin<&mut Self::ReceiverView<'v>>, value: T) -> T
    where
        Self: 'v,
        T: 'v + Copy;
    fn outliving_data<'v, 'x>(view: Pin<&mut Self::ReceiverView<'v>>, data: &'x [u8]) -> usize
    where
        Self: 'v,
        'x: 'v;
    /// And those relating the view's lifetimes, or capturing them all.
    fn related<'s, 'v>(view: Pin<&'s mut Self::ReceiverView<'v>>) -> usize
    where
        Self: 'v,
        'v: 's;
    fn related_inline<'s, 'v: 's>(view: Pin<&'s mut Self::ReceiverView<'v>>) -> usize
    where
        Self: 'v;
    fn listed<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> impl Iterator<Item = u32>
    where
        Self: 'v;
    /// A named borrow that nothing else needs changes nothing.
    fn sized<'s, 'v>(view: Pin<&'s mut Self::ReceiverView<'v>>) -> usize
    where
        Self: 'v;
}
impl Shape for Direct<Bins> {
    fn push_all<'v, I: IntoIterator<Item = u32>>(
        view: Pin<&mut DirectView<'v, Bins>>,
        values: I,
    ) -> usize
    where
        Self: 'v,
    {
        let bins = view.get_pin_mut().get_mut();
        bins.1.extend(values);
        bins.1.len()
    }
    fn first_of<'v, 'x, const N: usize>(
        _: Pin<&mut DirectView<'v, Bins>>,
        values: &'x [u32; N],
    ) -> &'x u32
    where
        Self: 'v,
    {
        &values[0]
    }
    fn named<'v>(_: Pin<&mut DirectView<'v, Bins>>, name: &'static str) -> &'static str
    where
        Self: 'v,
    {
        name
    }
    fn default_of<'v, D: Default + std::fmt::Debug>(_: Pin<&mut DirectView<'v, Bins>>) -> String
    where
        Self: 'v,
    {
        format!("{:?}", D::default())
    }
    fn converted<'v>(_: Pin<&mut DirectView<'v, Bins>>, value: impl Into<u32>) -> u32
    where
        Self: 'v,
    {
        value.into()
    }
    fn last<'s, 'v>(view: Pin<&'s mut DirectView<'v, Bins>>) -> Option<&'s u32>
    where
        Self: 'v,
        'v: 's,
    {
        view.get_pin_mut().get_mut().1.last()
    }
    fn marked<'v>(_: Pin<&mut DirectView<'v, Bins>>, mark: &'v u32) -> &'v u32
    where
        Self: 'v,
    {
        mark
    }
    fn outliving<'v, T: 'v + Copy>(_: Pin<&mut DirectView<'v, Bins>>, value: T) -> T
    where
        Self: 'v,
    {
        value
    }
    fn outliving_too<'v, T>(_: Pin<&mut DirectView<'v, Bins>>, value: T) -> T
    where
        Self: 'v,
        T: 'v + Copy,
    {
        value
    }
    fn outliving_data<'v, 'x>(_: Pin<&mut DirectView<'v, Bins>>, data: &'x [u8]) -> usize
    where
        Self: 'v,
        'x: 'v,
    {
        data.len()
    }
    fn related<'s, 'v>(view: Pin<&'s mut DirectView<'v, Bins>>) -> usize
    where
        Self: 'v,
        'v: 's,
    {
        view.get_pin_mut().1.len()
    }
    fn related_inline<'s, 'v: 's>(view: Pin<&'s mut DirectView<'v, Bins>>) -> usize
    where
        Self: 'v,
    {
        view.get_pin_mut().1.len()
    }
    fn listed<'v>(view: Pin<&mut DirectView<'v, Bins>>) -> impl Iterator<Item = u32>
    where
        Self: 'v,
    {
        view.get_pin_mut().get_mut().1.clone().into_iter()
    }
    fn sized<'s, 'v>(view: Pin<&'s mut DirectView<'v, Bins>>) -> usize
    where
        Self: 'v,
    {
        view.get_pin_mut().1.len()
    }
}

#[call]
async fn shaping<F: Shape>(io: &mut impl LentAccess<Family = F>, kept: &mut Vec<String>) {
    let values = [4, 5];
    assert_eq!(io.push_all(values), 2);
    assert_eq!(io.first_of(&values), &4);
    assert_eq!(io.named("access"), "access");
    assert_eq!(io.default_of::<(u8, bool)>(), "(0, false)");
    assert_eq!(io.converted(7_u8), 7);
    assert_eq!(io.pattern((1, 2)), 3);
    assert_eq!(io.sized(), 2);
    let last = io.with(|mut view| {
        assert_eq!(view.push_all([6]), 3);
        assert_eq!(view.first_of(&values), &4);
        assert_eq!(view.named("view"), "view");
        assert_eq!(view.default_of::<u8>(), "0");
        assert_eq!(view.converted(8_u16), 8);
        assert_eq!(view.pattern((2, 3)), 5);
        assert_eq!(*view.marked(&9), 9);
        assert_eq!(view.outliving(1_u8), 1);
        assert_eq!(view.outliving_too(2_u8), 2);
        assert_eq!(view.outliving_data(b"ab"), 2);
        assert_eq!(view.related(), 3);
        assert_eq!(view.related_inline(), 3);
        assert_eq!(view.sized(), 3);
        assert_eq!(view.listed().sum::<u32>(), 15);
        view.last().copied()
    });
    kept.push(format!("{last:?}"));
}

#[test]
fn method_parameters_and_lifetimes_pass_through() {
    let mut bins = Bins::default();
    let mut kept = Vec::new();
    {
        let mut call = pin!(LentCall::<Direct<Bins>, _>::new(Shaping::new(&mut kept)));
        assert!(matches!(
            poll_on(call.as_mut(), &mut bins),
            Poll::Ready(ControlFlow::Break(()))
        ));
    }
    assert_eq!(kept, ["Some(6)"]);
}

/// A capability can mix poll methods and synchronous ones.
#[capability]
pub trait Buffer: ReceiverFamily {
    fn poll_take<'v>(view: Pin<&mut Self::ReceiverView<'v>>, cx: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v;
    fn remaining<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> usize
    where
        Self: 'v;
}
impl Buffer for Direct<Items> {
    fn poll_take<'v>(view: Pin<&mut DirectView<'v, Items>>, _: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v,
    {
        let items = view.get_pin_mut().get_mut();
        items.0.pop_front().map_or(Poll::Pending, Poll::Ready)
    }
    fn remaining<'v>(view: Pin<&mut DirectView<'v, Items>>) -> usize
    where
        Self: 'v,
    {
        view.get_pin_mut().0.len()
    }
}

#[call]
async fn taking(io: &mut impl LentAccess<Family: Buffer>) -> (u32, usize) {
    // Without a releasable view, a release point completes at once.
    io.release_view().await;
    let taken = io.take().await;
    (taken, io.remaining())
}

#[test]
fn poll_methods_wait_and_views_may_change_between_polls() {
    let mut empty = Items::default();
    let mut full = Items(VecDeque::from([1, 2]));
    let mut call = pin!(LentCall::<Direct<Items>, _>::new(Taking::new()));
    assert!(matches!(poll_on(call.as_mut(), &mut empty), Poll::Pending));
    assert!(matches!(
        poll_on(call.as_mut(), &mut full),
        Poll::Ready(ControlFlow::Break((1, 1)))
    ));
}

/// Where a destructor finds itself decides whether it has a view.
struct Farewell<'x, A: LentAccess<Family: Counter>> {
    io: &'x mut A,
    seen: Rc<Cell<Option<Option<u32>>>>,
}
impl<A: LentAccess<Family: Counter>> Drop for Farewell<'_, A> {
    fn drop(&mut self) {
        self.seen
            .set(Some(self.io.try_with(|mut view| view.add(1))));
    }
}
#[call]
async fn saying_farewell<F: Counter, A: LentAccess<Family = F>>(
    io: &mut A,
    seen: Rc<Cell<Option<Option<u32>>>>,
    wait: bool,
) {
    let farewell = Farewell { io, seen };
    if wait {
        pending::<()>().await;
    }
    drop(farewell);
}

#[test]
fn a_destructor_in_a_poll_has_its_view() {
    let seen = Rc::new(Cell::new(None));
    let mut total = Total::default();
    let mut call = pin!(LentCall::<Direct<Total>, _>::new(SayingFarewell::new(
        seen.clone(),
        false
    )));
    assert!(matches!(
        poll_on(call.as_mut(), &mut total),
        Poll::Ready(ControlFlow::Break(()))
    ));
    assert_eq!(seen.get(), Some(Some(1)));
}

#[test]
fn a_destructor_outside_a_poll_has_no_view() {
    let seen = Rc::new(Cell::new(None));
    let mut total = Total::default();
    let mut call = Box::pin(LentCall::<Direct<Total>, _>::new(SayingFarewell::new(
        seen.clone(),
        true,
    )));
    assert!(matches!(poll_on(call.as_mut(), &mut total), Poll::Pending));
    assert_eq!(seen.get(), None);
    drop(call);
    assert_eq!(seen.get(), Some(None));
    assert_eq!(total.0, 0);
}

struct FarewellToBoth<'x, A: LentAccess<Family: Counter>, B: LentAccess<Family: Counter>> {
    first: &'x mut A,
    second: &'x mut B,
    seen: Rc<Cell<Option<Option<u32>>>>,
}
impl<A, B> Drop for FarewellToBoth<'_, A, B>
where
    A: LentAccess<Family: Counter>,
    B: LentAccess<Family: Counter>,
{
    fn drop(&mut self) {
        let both = (&mut *self.first, &mut *self.second)
            .try_with(|(mut first, mut second)| first.add(1) + second.add(1));
        self.seen.set(Some(both));
    }
}
#[call]
async fn saying_farewell_to_both<F: Counter, G: Counter, A, B>(
    first: &mut A,
    second: &mut B,
    seen: Rc<Cell<Option<Option<u32>>>>,
    wait: bool,
) where
    A: LentAccess<Family = F>,
    B: LentAccess<Family = G>,
{
    let farewell = FarewellToBoth {
        first,
        second,
        seen,
    };
    if wait {
        pending::<()>().await;
    }
    drop(farewell);
}

#[test]
fn several_accesses_have_their_views_only_in_a_poll() {
    for (wait, expected) in [(false, Some(2)), (true, None)] {
        let seen = Rc::new(Cell::new(None));
        let (mut first, mut second) = (Total::default(), Total::default());
        let mut call = Box::pin(LentCall::<(Direct<Total>, Direct<Total>), _>::new(
            SayingFarewellToBoth::new(seen.clone(), wait),
        ));
        let mut first_view = pin!(Pin::new(&mut first).view());
        let mut second_view = pin!(Pin::new(&mut second).view());
        let poll = call.as_mut().poll_with_views(
            (first_view.as_mut(), second_view.as_mut()),
            &mut Context::from_waker(Waker::noop()),
        );
        assert_eq!(poll.is_ready(), !wait);
        drop(call);
        assert_eq!(seen.get(), Some(expected));
    }
}

/// A scoped thread can hold a slot, but the view stays on the polling thread,
/// so `try_with` panics there: it is not outside a poll.
struct Elsewhere;
impl Operation<Direct<Total>> for Elsewhere {
    type Output = String;
    type Yield = Infallible;
}
impl<'s> Start<'s, LentSlot<'s, Direct<Total>>> for Elsewhere {
    type Fut = impl Future<Output = String>;
    fn start(self, mut io: LentSlot<'s, Direct<Total>>, _: Emitter<'s, Infallible>) -> Self::Fut {
        async move {
            let panic = thread::scope(|scope| scope.spawn(|| io.try_with(|_| ())).join());
            let panic = panic.expect_err("a view used on another thread");
            panic.downcast_ref::<&str>().unwrap().to_string()
        }
    }
}

#[test]
fn a_view_stays_on_the_polling_thread() {
    let mut total = Total::default();
    let mut call = pin!(LentCall::<Direct<Total>, _>::new(Elsewhere));
    let Poll::Ready(ControlFlow::Break(panic)) = poll_on(call.as_mut(), &mut total) else {
        panic!("the operation does not wait");
    };
    assert_eq!(
        panic,
        "a view lent to a call used outside the thread polling it"
    );
}

struct Insistent<'x, A: LentAccess<Family: Counter>>(&'x mut A);
impl<A: LentAccess<Family: Counter>> Drop for Insistent<'_, A> {
    fn drop(&mut self) {
        self.0.add(1);
    }
}
#[call]
async fn insisting<F: Counter>(io: &mut impl LentAccess<Family = F>) {
    let _insistent = Insistent(io);
    pending::<()>().await;
}

#[test]
#[should_panic(expected = "a lent view used outside a poll")]
fn with_outside_a_poll_panics() {
    let mut total = Total::default();
    let mut call = pin!(LentCall::<Direct<Total>, _>::new(Insisting::new()));
    assert!(matches!(poll_on(call.as_mut(), &mut total), Poll::Pending));
}

/// One step on each of four receivers at once.
#[call]
async fn all_four<A, B, C, D>(
    a: &mut impl LentAccess<Family = A>,
    b: &mut impl LentAccess<Family = B>,
    c: &mut impl LentAccess<Family = C>,
    d: &mut impl LentAccess<Family = D>,
) -> (u32, Option<u32>)
where
    A: Counter,
    B: Counter,
    C: Counter,
    D: Counter,
{
    let sum = (&mut *a, &mut *b, &mut *c, &mut *d)
        .with(|(mut a, mut b, mut c, mut d)| a.add(1) + b.add(2) + c.add(3) + d.add(4));
    let again = (&mut *a, &mut *b, &mut *c, &mut *d)
        .try_with(|(mut a, mut b, mut c, mut d)| a.add(1) + b.add(1) + c.add(1) + d.add(1));
    (sum, again)
}

#[test]
fn four_accesses_at_once() {
    let [mut a, mut b, mut c, mut d] = [(); 4].map(|()| Total::default());
    type Four = (Direct<Total>, Direct<Total>, Direct<Total>, Direct<Total>);
    let mut call = pin!(LentCall::<Four, _>::new(AllFour::new()));
    let mut views = (
        pin!(Pin::new(&mut a).view()),
        pin!(Pin::new(&mut b).view()),
        pin!(Pin::new(&mut c).view()),
        pin!(Pin::new(&mut d).view()),
    );
    let poll = call.as_mut().poll_with_views(
        (
            views.0.as_mut(),
            views.1.as_mut(),
            views.2.as_mut(),
            views.3.as_mut(),
        ),
        &mut Context::from_waker(Waker::noop()),
    );
    assert!(matches!(
        poll,
        Poll::Ready(ControlFlow::Break((10, Some(14))))
    ));
}

/// A view of the same counter, as a composed access.
struct SameCounter;
impl Compose<Direct<Total>> for SameCounter {
    type Family = Direct<Total>;
    fn compose<'scope, 'parent>(
        self: Pin<&'scope mut Self>,
        parent: Pin<&'scope mut DirectView<'parent, Total>>,
    ) -> DirectView<'scope, Total>
    where
        Direct<Total>: 'parent,
        Direct<Total>: 'scope,
        'parent: 'scope,
    {
        DirectView(parent.get_pin_mut())
    }
}

/// A plain operation, to run on a lent access.
#[call]
async fn plain_add<F: Counter>(io: &mut impl ReceiverAccess<Family = F>, by: u32) -> u32 {
    poll_fn(|cx| io.poll_access(cx, |access, _| Poll::Ready(F::add(access.into_pin(), by)))).await
}

struct Composing;
impl Operation<(Direct<Total>, Direct<Total>)> for Composing {
    type Output = u32;
    type Yield = Infallible;
}
impl<'s> Start<'s, (LentSlot<'s, Direct<Total>>, LentSlot<'s, Direct<Total>>)> for Composing {
    type Fut = impl Future<Output = u32>;
    fn start(
        self,
        (mut io, mut other): (LentSlot<'s, Direct<Total>>, LentSlot<'s, Direct<Total>>),
        _: Emitter<'s, Infallible>,
    ) -> Self::Fut {
        async move {
            assert_eq!(format!("{io:?}"), "LentSlot { .. }");
            let mut composed = io.compose(SameCounter);
            composed.add(1);
            composed.add_twice(2).await;
            composed.plain_add(3).await;
            (io.compose(SameCounter), &mut other).with(|(mut first, mut second)| {
                second.add(10);
                first.add(4)
            })
        }
    }
}

#[test]
fn compositions_of_lent_accesses_are_lent() {
    let (mut first, mut second) = (Total::default(), Total::default());
    let mut call = pin!(LentCall::<(Direct<Total>, Direct<Total>), _>::new(
        Composing
    ));
    let mut first_view = pin!(Pin::new(&mut first).view());
    let mut second_view = pin!(Pin::new(&mut second).view());
    assert!(matches!(
        call.as_mut().poll_with_views(
            (first_view.as_mut(), second_view.as_mut()),
            &mut Context::from_waker(Waker::noop()),
        ),
        Poll::Ready(ControlFlow::Break(12))
    ));
}

/// Tuples of receivers nest, and a loan can stand in for a pinned view.
struct Nested;
type NestedFamilies = ((Direct<Total>, Direct<Items>), Direct<Total>);
impl Operation<NestedFamilies> for Nested {
    type Output = u32;
    type Yield = Infallible;
}
type NestedSlots<'s> = (
    (LentSlot<'s, Direct<Total>>, LentSlot<'s, Direct<Items>>),
    LentSlot<'s, Direct<Total>>,
);
impl<'s> Start<'s, NestedSlots<'s>> for Nested {
    type Fut = impl Future<Output = u32>;
    fn start(
        self,
        ((mut first, mut items), mut last): NestedSlots<'s>,
        _: Emitter<'s, Infallible>,
    ) -> Self::Fut {
        async move {
            let item = items.next().await;
            (&mut first, &mut last).with(|(mut first, mut last)| first.add(item) + last.add(item))
        }
    }
}

#[test]
fn views_follow_the_shape_of_the_receivers() {
    let (mut first, mut last) = (Total::default(), Total::default());
    let mut items = Items(VecDeque::from([2]));
    let mut call = pin!(LentCall::<NestedFamilies, _>::new(Nested));
    let mut first_view = pin!(Pin::new(&mut first).view());
    let mut items_view = pin!(Pin::new(&mut items).view());
    let mut last_view = pin!(Pin::new(&mut last).view());
    assert!(matches!(
        call.as_mut().poll_return_with_views(
            (
                (first_view.as_mut(), ViewLoan::new(items_view.as_mut())),
                last_view.as_mut(),
            ),
            &mut Context::from_waker(Waker::noop()),
        ),
        Poll::Ready(4)
    ));
}

/// Several accesses, some lent and some not, are all slots of a lent call.
#[call]
async fn mixed<F, G>(
    plain: &mut impl ReceiverAccess<Family = F>,
    lent: &mut impl nitori_call::LentAccess<Family = G>,
) -> u32
where
    F: Queue,
    G: Counter,
{
    let item = plain.next().await;
    lent.add(item)
}

#[test]
fn a_lent_call_runs_operations_with_plain_accesses_too() {
    let mut items = Items(VecDeque::from([6]));
    let mut total = Total::default();
    let mut call = pin!(LentCall::<(Direct<Items>, Direct<Total>), _>::new(
        Mixed::new()
    ));
    let mut items_view = pin!(Pin::new(&mut items).view());
    let mut total_view = pin!(Pin::new(&mut total).view());
    assert!(matches!(
        call.as_mut().poll_with_views(
            (items_view.as_mut(), total_view.as_mut()),
            &mut Context::from_waker(Waker::noop()),
        ),
        Poll::Ready(ControlFlow::Break(6))
    ));
}

/// A plain access in a lent call still waits for its own events, while the
/// lent one stays synchronous on every poll.
#[call]
async fn gated<F, G>(
    gate: &mut impl ReceiverAccess<Family = F>,
    total: &mut impl LentAccess<Family = G>,
) -> u32
where
    F: Queue,
    G: Counter,
{
    total.add(1);
    let item = gate.next().await;
    gate.release_view().await;
    total.add(item)
}

#[test]
fn a_plain_access_of_a_lent_call_can_wait() {
    let mut items = Items::default();
    let mut total = Total::default();
    let mut call = pin!(LentCall::<(Direct<Items>, Direct<Total>), _>::new(
        Gated::new()
    ));
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut items_view = pin!(Pin::new(&mut items).view());
        let mut total_view = pin!(Pin::new(&mut total).view());
        assert!(matches!(
            call.as_mut()
                .poll_with_views((items_view.as_mut(), total_view.as_mut()), &mut cx),
            Poll::Pending
        ));
    }
    assert_eq!(total.0, 1);
    items.0.push_back(6);
    let mut items_view = pin!(Pin::new(&mut items).view());
    let mut total_view = pin!(Pin::new(&mut total).view());
    assert!(matches!(
        call.as_mut()
            .poll_with_views((items_view.as_mut(), total_view.as_mut()), &mut cx),
        Poll::Ready(ControlFlow::Break(7))
    ));
}

/// A lent access goes to operations written for either kind, alone or mixed.
#[call]
async fn plain_step<F: Queue>(io: &mut impl ReceiverAccess<Family = F>) -> u32 {
    io.next().await
}
#[call]
async fn through_both<F, G>(
    queue: &mut impl nitori_call::LentAccess<Family = F>,
    total: &mut impl nitori_call::LentAccess<Family = G>,
) -> u32
where
    F: Queue,
    G: Counter,
{
    let first = queue.plain_step().await;
    first + mixed(queue, total).await
}

#[test]
fn a_lent_access_reaches_plain_and_mixed_operations() {
    let mut items = Items(VecDeque::from([2, 3]));
    let mut total = Total::default();
    let mut call = pin!(LentCall::<(Direct<Items>, Direct<Total>), _>::new(
        ThroughBoth::new()
    ));
    let mut items_view = pin!(Pin::new(&mut items).view());
    let mut total_view = pin!(Pin::new(&mut total).view());
    assert!(matches!(
        call.as_mut().poll_with_views(
            (items_view.as_mut(), total_view.as_mut()),
            &mut Context::from_waker(Waker::noop()),
        ),
        Poll::Ready(ControlFlow::Break(5))
    ));
}

/// The access's bounds decide, not the text of the signature.
pub struct LentAccessLog(u32);
#[call(sync)]
async fn not_lent(_: &mut impl ReceiverAccess<Family: Queue>, log: LentAccessLog) -> u32 {
    log.0
}
#[call]
async fn both_bounds<F, A>(io: &mut A) -> u32
where
    F: Counter,
    A: ReceiverAccess<Family = F> + nitori_call::LentAccess,
{
    io.add(1)
}

#[test]
fn only_the_access_bounds_make_an_operation_lent() {
    let mut items = Items::default();
    assert_eq!(items.sync_not_lent_unpin(LentAccessLog(7)), 7);
    let mut total = Total::default();
    let mut call = pin!(LentCall::<Direct<Total>, _>::new(BothBounds::new()));
    assert!(matches!(
        poll_on(call.as_mut(), &mut total),
        Poll::Ready(ControlFlow::Break(1))
    ));
}

#[test]
fn a_lent_call_shows_whether_it_finished() {
    let mut total = Total::default();
    let mut call = pin!(LentCall::<Direct<Total>, _>::new(BothBounds::new()));
    assert_eq!(format!("{call:?}"), "LentCall { terminated: false, .. }");
    assert!(poll_on(call.as_mut(), &mut total).is_ready());
    assert_eq!(format!("{call:?}"), "LentCall { terminated: true, .. }");
}

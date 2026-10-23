mod support;

use nitori_call_prototype::{
    BorrowedScope, Call, Direct, DirectView, DynCall, Emitter, Receiver, ReceiverAccess,
    ReceiverFamily, ReceiverSlot,
};
use std::{
    cell::Cell,
    convert::Infallible,
    future::poll_fn,
    ops::ControlFlow,
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
};
use support::*;

type Memory<'a> = Direct<&'a [u8]>;
type Record = Result<usize, ReadError<Infallible>>;
type Pair = Result<[u8; 2], ReadError<Infallible>>;
type Boxed<'a, F, R> = Pin<Box<dyn DynCall<F, Infallible, R> + 'a>>;

async fn read_record<A>(io: &mut A, output: &mut Vec<u8>) -> Result<usize, ReadError<Infallible>>
where
    A: ReceiverAccess,
    A::Family: Read<Error = Infallible>,
{
    let [length] = read_array::<1, _>(io).await?;
    read_exact(io, output, length.into()).await?;
    Ok(length.into())
}

// A root operation: the slot and emitter, then its own arguments.
async fn read_record_root<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    _: Emitter<'_, Infallible>,
    output: &mut Vec<u8>,
) -> Result<usize, ReadError<Infallible>> {
    read_record(&mut io, output).await
}

fn poll<F: ReceiverFamily, Y, R>(
    call: Pin<&mut (impl DynCall<F, Y, R> + ?Sized)>,
    host: Pin<&mut impl Receiver<Family = F>>,
) -> Poll<ControlFlow<R, Y>> {
    call.poll_with(
        pin!(BorrowedScope::new(host)),
        &mut Context::from_waker(Waker::noop()),
    )
}

#[test]
fn read_record_from_memory() {
    let mut input = b"\x03cat\x03dog".as_slice();
    let mut output = Vec::new();
    {
        let mut call = pin!(Call::new((read_record_root::<Memory>, &mut output)));
        assert!(matches!(
            poll(call.as_mut(), Pin::new(&mut input)),
            Poll::Ready(ControlFlow::Break(Ok(3)))
        ));
    }
    assert_eq!(input, b"\x03dog");
    assert_eq!(output, b"cat");
}

async fn read_word<A>(io: &mut A) -> Result<[u8; 2], ReadError<Infallible>>
where
    A: ReceiverAccess,
    A::Family: Read<Error = Infallible>,
{
    read_array::<2, _>(io).await
}

async fn read_limited_word<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    _: Emitter<'_, Infallible>,
    (): (),
) -> Result<[u8; 2], ReadError<Infallible>> {
    let [length] = read_array::<1, _>(&mut io).await?;
    read_word(&mut io.compose(ReadLimit(length.into()))).await
}

#[test]
fn composition_keeps_a_child_inside_its_record() {
    // Without the boundary, a two-byte parser consumes the next record's length.
    let mut input = b"\x01A\x02BC".as_slice();
    let mut first = pin!(Call::new((read_limited_word::<Memory>, ())));
    assert!(matches!(
        poll(first.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break(Err(ReadError::Incomplete {
            completed: 1
        })))
    ));
    assert_eq!(input, b"\x02BC");
    let mut second = pin!(Call::new((read_limited_word::<Memory>, ())));
    assert!(matches!(
        poll(second.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break(Ok(word))) if word == *b"BC"
    ));
}

async fn read_erased_word<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    _: Emitter<'_, Infallible>,
    (): (),
) -> Result<[u8; 2], ReadError<Infallible>> {
    // An operation that accepts only slots, given an erased composed access.
    async fn word_from_slot<G: Read<Error = Infallible>>(
        io: &mut ReceiverSlot<'_, G>,
    ) -> Result<[u8; 2], ReadError<Infallible>> {
        read_array::<2, _>(io).await
    }
    let mut limited = io.compose(ReadLimit(2));
    word_from_slot(&mut limited.erase()).await
}

#[test]
fn erased_composition_is_a_slot() {
    let mut input = b"XYZ".as_slice();
    let mut call = pin!(Call::new((read_erased_word::<Memory>, ())));
    assert!(matches!(
        poll(call.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break(Ok(word))) if word == *b"XY"
    ));
    assert_eq!(input, b"Z");
}

#[test]
fn a_call_may_use_a_different_host_on_each_poll() {
    // The call is stored apart from its hosts and resumes with whichever is lent.
    let mut output = Vec::new();
    let mut second = Feed::new(b"t!");
    {
        let mut call = pin!(Call::new((read_record_root::<Direct<Feed>>, &mut output)));
        let mut first = Feed::new(b"\x03ca");
        assert!(poll(call.as_mut(), Pin::new(&mut first)).is_pending());
        assert!(first.waker.is_some());
        assert!(matches!(
            poll(call.as_mut(), Pin::new(&mut second)),
            Poll::Ready(ControlFlow::Break(Ok(3)))
        ));
    }
    assert_eq!(second.data, b"!");
    assert_eq!(output, b"cat");
}

struct Connection {
    input: Feed,
    reader: Boxed<'static, Direct<Feed>, Record>,
}

async fn read_into_owned<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    _: Emitter<'_, Infallible>,
    output: Rc<Cell<Vec<u8>>>,
) -> Result<usize, ReadError<Infallible>> {
    let mut buffer = Vec::new();
    let length = read_record(&mut io, &mut buffer).await?;
    output.set(buffer);
    Ok(length)
}

#[test]
fn a_call_is_stored_beside_the_resource_it_uses() {
    let output = Rc::new(Cell::new(Vec::new()));
    let mut connection = Connection {
        input: Feed::new(b"\x02h"),
        reader: Box::pin(Call::new((read_into_owned::<Direct<Feed>>, output.clone()))),
    };
    let Connection { input, reader } = &mut connection;
    assert!(poll(reader.as_mut(), Pin::new(input)).is_pending());
    connection.input.data.extend(*b"i");
    let Connection { input, reader } = &mut connection;
    assert!(matches!(
        poll(reader.as_mut(), Pin::new(input)),
        Poll::Ready(ControlFlow::Break(Ok(2)))
    ));
    assert_eq!(output.take(), b"hi");
}

/// A buffer that counts how often a view is constructed.
struct Counted<'a> {
    input: &'a [u8],
    views: &'a Cell<usize>,
}
impl<'a> Receiver for Counted<'a> {
    type Family = Memory<'a>;
    fn view<'visit>(self: Pin<&'visit mut Self>) -> DirectView<'visit, &'a [u8]>
    where
        Self::Family: 'visit,
    {
        let this = self.get_mut();
        this.views.set(this.views.get() + 1);
        DirectView(Pin::new(&mut this.input))
    }
}

async fn read_pair<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    _: Emitter<'_, Infallible>,
    (): (),
) -> Result<[u8; 2], ReadError<Infallible>> {
    let [first] = read_array::<1, _>(&mut io).await?;
    let [second] = read_array::<1, _>(&mut io).await?;
    Ok([first, second])
}

#[test]
fn an_executor_lends_one_view_to_several_calls_in_one_poll() {
    let views = Cell::new(0);
    let mut resource = Counted {
        input: b"abcd",
        views: &views,
    };
    let mut calls: Vec<Boxed<'_, Memory, Pair>> = vec![
        Box::pin(Call::new((read_pair::<Memory>, ()))),
        Box::pin(Call::new((read_pair::<Memory>, ()))),
    ];
    // One scope for the whole poll: the view is constructed on first use.
    let mut scope = pin!(BorrowedScope::new(Pin::new(&mut resource)));
    let mut cx = Context::from_waker(Waker::noop());
    let results: Vec<_> = calls
        .iter_mut()
        .map(|call| call.as_mut().poll_with(scope.as_mut(), &mut cx))
        .collect();
    assert!(matches!(results[0], Poll::Ready(ControlFlow::Break(Ok(pair))) if pair == *b"ab"));
    assert!(matches!(results[1], Poll::Ready(ControlFlow::Break(Ok(pair))) if pair == *b"cd"));
    assert_eq!(views.get(), 1);
}

async fn read_chunks<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    mut emitter: Emitter<'_, Vec<u8>>,
    total: usize,
) -> Result<usize, ReadError<Infallible>> {
    let mut completed = 0;
    while completed < total {
        let mut chunk = Vec::new();
        read_exact(&mut io, &mut chunk, 2.min(total - completed)).await?;
        completed += chunk.len();
        emitter.emit(chunk).await;
    }
    Ok(completed)
}

#[test]
fn emitted_items_precede_the_result() {
    let mut input = b"abcde".as_slice();
    let mut call = pin!(Call::new((read_chunks::<Memory>, 5)));
    let mut events = Vec::new();
    loop {
        match poll(call.as_mut(), Pin::new(&mut input)) {
            Poll::Ready(ControlFlow::Continue(chunk)) => events.push(chunk),
            Poll::Ready(ControlFlow::Break(result)) => {
                assert_eq!(result, Ok(5));
                break;
            }
            Poll::Pending => panic!("memory input does not wait"),
        }
    }
    assert_eq!(events, [b"ab".to_vec(), b"cd".to_vec(), b"e".to_vec()]);
}

#[test]
fn return_mode_discards_items_within_one_scope() {
    let views = Cell::new(0);
    let mut resource = Counted {
        input: b"abcde",
        views: &views,
    };
    let mut call = pin!(Call::new((read_chunks::<Memory>, 5)));
    let result = call.as_mut().poll_return_with(
        pin!(BorrowedScope::new(Pin::new(&mut resource))),
        &mut Context::from_waker(Waker::noop()),
    );
    assert!(matches!(result, Poll::Ready(Ok(5))));
    assert_eq!(views.get(), 1);
}

async fn sum_child_chunks<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    _: Emitter<'_, Infallible>,
    (): (),
) -> (usize, Result<usize, ReadError<Infallible>>) {
    // A nested call with its own item type, driven through the parent's access.
    let mut child = pin!(Call::new((read_chunks::<F>, 4)));
    let mut chunks = 0;
    let result = loop {
        match poll_fn(|cx| io.poll_call(child.as_mut(), cx)).await {
            ControlFlow::Continue(_) => chunks += 1,
            ControlFlow::Break(result) => break result,
        }
    };
    (chunks, result)
}

#[test]
fn a_parent_observes_the_events_of_a_nested_call() {
    let mut input = b"wxyz".as_slice();
    let mut call = pin!(Call::new((sum_child_chunks::<Memory>, ())));
    assert!(matches!(
        poll(call.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break((2, Ok(4))))
    ));
}

async fn panic_after_read<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    _: Emitter<'_, Infallible>,
    (): (),
) {
    let _ = read_array::<1, _>(&mut io).await;
    panic!("operation failed");
}

#[test]
fn a_call_is_poisoned_by_a_panic() {
    let mut input = b"a".as_slice();
    let mut call = Box::pin(Call::new((panic_after_read::<Memory>, ())));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        poll(call.as_mut(), Pin::new(&mut input))
    }));
    assert!(unwound.is_err());
    let again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        poll(call.as_mut(), Pin::new(&mut input))
    }));
    let message = again.unwrap_err();
    assert_eq!(
        message.downcast_ref::<&str>(),
        Some(&"call polled after a panic")
    );
}

async fn read_on_another_thread<F: Read<Error = Infallible>>(
    mut io: ReceiverSlot<'_, F>,
    _: Emitter<'_, Infallible>,
    (): (),
) -> bool {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_ready(read_array::<1, _>(&mut io))
                }))
                .is_err()
            })
            .join()
            .unwrap()
    })
}

#[test]
fn a_slot_refuses_other_threads() {
    let mut input = b"a".as_slice();
    let mut call = pin!(Call::new((read_on_another_thread::<Memory>, ())));
    assert!(matches!(
        poll(call.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break(true))
    ));
    assert_eq!(input, b"a");
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn a_call_over_a_generic_family_is_send() {
    fn build<F: Read<Error = Infallible> + 'static>() {
        let mut output = Vec::new();
        let call = Call::new((read_record_root::<F>, &mut output));
        assert_send(&call);
    }
    build::<Memory<'static>>();
}

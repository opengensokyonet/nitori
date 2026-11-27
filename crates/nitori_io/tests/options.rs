//! Write options: what each segment is written with, through the write
//! calls, lending, and the adapters that map options into a sink's.
#![feature(impl_trait_in_assoc_type)]
use bytes::{Buf, BufMut, Bytes};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use nitori_call::{ReceiverAccess, call, family_receiver};
use nitori_io::{
    Combine, Filled, LentBuffer, LentOutcome, LentSegment, WriteSegment,
    adapters::{FixedOptions, MapOptions, MappedOptions, MappedOptionsParts, OptionsMapping},
    calls::{CopyTotal, WriteAllError, copy_chunks},
    prelude::*,
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

/// How urgently a segment is to be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Priority(u8);

/// Whether the bytes arrived early; merges to "some".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Early(bool);
impl Combine for Early {
    fn combine(self, next: Self) -> Self {
        Early(self.0 || next.0)
    }
}

/// A sink that accepts or lends at most two bytes at a time and records
/// each segment it keeps with the options it was written with.
struct Recorder<O> {
    segments: Vec<(Vec<u8>, O)>,
    /// How many writes still answer Pending before accepting.
    pending: usize,
}
impl<O> Default for Recorder<O> {
    fn default() -> Self {
        Self {
            segments: Vec::new(),
            pending: 0,
        }
    }
}
family_receiver!(impl [O] for Recorder<O>);
impl<O: Clone + Unpin> Write<O> for Recorder<O> {
    type Error = !;
    type Report = ();
    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        input: &mut B,
        options: &O,
    ) -> Poll<Result<Option<WriteSegment<()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if this.pending > 0 {
            this.pending -= 1;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let Some(count) = NonZeroUsize::new(input.remaining().min(2)) else {
            return Poll::Ready(Ok(None));
        };
        let mut bytes = vec![0; count.get()];
        input.copy_to_slice(&mut bytes);
        this.segments.push((bytes, options.clone()));
        Poll::Ready(Ok(Some(WriteSegment { count, report: () })))
    }
}
impl<O: Clone + Unpin> WriteLent<O> for Recorder<O> {
    type Buffer<'l> = &'l mut Vec<u8>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<Result<Option<LentOutcome<(), T, A>>, !>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<&'l mut Vec<u8>>) -> Result<Filled<T, O>, A>,
    {
        let this = host.get_pin_mut().get_mut();
        let granted = wanted.min(nz(2));
        let mut storage = Vec::new();
        let mut lent = LentBuffer::new(&mut storage, granted).expect("a vector grows");
        let result = fill(&mut lent);
        let kept = NonZeroUsize::new(storage.len()).filter(|count| *count <= granted);
        let outcome = match (result, kept) {
            (Ok(Filled { value, options }), Some(count)) => {
                this.segments.push((storage, options));
                LentOutcome::Kept(LentSegment {
                    count,
                    report: (),
                    value,
                })
            }
            (Ok(Filled { value, .. }), None) => LentOutcome::NotKept(value),
            (Err(abandoned), _) => LentOutcome::GaveUp(abandoned),
        };
        Poll::Ready(Ok(Some(outcome)))
    }
}

fn segment<O>(bytes: &[u8], options: O) -> (Vec<u8>, O) {
    (bytes.to_vec(), options)
}

/// Lend `sink` for up to eight bytes to a fill that puts `bytes` and returns
/// `options`, or gives up.
fn lend<S, O>(sink: &mut S, bytes: &[u8], options: Option<O>) -> LentOutcome<(), (), ()>
where
    S: nitori_call::Receiver + Unpin,
    S::Family: WriteLent<O, Report = (), Error = !>,
{
    let mut cx = Context::from_waker(Waker::noop());
    let polled = Pin::new(sink).poll_write_lent(&mut cx, nz(8), |lent| {
        lent.buffer().put_slice(bytes);
        options
            .map(|options| Filled { value: (), options })
            .ok_or(())
    });
    let Poll::Ready(Ok(Some(outcome))) = polled else {
        panic!("the recorder lends at once");
    };
    outcome
}

// The write calls.

#[test]
fn write_all_writes_every_segment_with_its_options() {
    let mut sink = Recorder::default();
    sink.sync_write_all_unpin(&b"abcde"[..], Priority(7))
        .unwrap();
    assert_eq!(
        sink.segments,
        [
            segment(b"ab", Priority(7)),
            segment(b"cd", Priority(7)),
            segment(b"e", Priority(7)),
        ]
    );
}

#[test]
fn the_numeric_and_array_writes_write_with_their_options() {
    let mut sink = Recorder::default();
    sink.sync_write_le_unpin(0x0201_u16, Priority(1)).unwrap();
    sink.sync_write_be_unpin(0x0304_u16, Priority(2)).unwrap();
    sink.sync_write_array_unpin(*b"xyz", Priority(3)).unwrap();
    assert_eq!(
        sink.segments,
        [
            segment(&[1, 2], Priority(1)),
            segment(&[3, 4], Priority(2)),
            segment(b"xy", Priority(3)),
            segment(b"z", Priority(3)),
        ]
    );
}

// Lending.

#[test]
fn a_lending_sink_writes_the_kept_bytes_with_the_options_the_fill_returns() {
    let mut sink = Recorder::default();
    let outcome = lend(&mut sink, b"ab", Some(Priority(4)));
    assert!(matches!(outcome, LentOutcome::Kept(LentSegment { count, .. }) if count.get() == 2));
    assert_eq!(sink.segments, [segment(b"ab", Priority(4))]);
}

#[test]
fn a_fill_that_gives_up_or_puts_nothing_leaves_its_options_unrecorded() {
    let mut sink = Recorder::<Priority>::default();
    assert!(matches!(
        lend(&mut sink, b"ab", None),
        LentOutcome::GaveUp(())
    ));
    assert!(matches!(
        lend(&mut sink, b"", Some(Priority(5))),
        LentOutcome::NotKept(())
    ));
    assert!(sink.segments.is_empty());
}

// Mapping the options.

/// Lends one of two kept priorities by whether the bytes arrived early.
struct ByArrival {
    early: Priority,
    late: Priority,
}
impl Default for ByArrival {
    fn default() -> Self {
        Self {
            early: Priority(0),
            late: Priority(9),
        }
    }
}
impl MapOptions<Early> for ByArrival {
    type Options = Priority;
    fn map_options<'a>(&'a mut self, options: &'a Early) -> &'a Priority {
        if options.0 { &self.early } else { &self.late }
    }
}

#[test]
fn an_options_mapping_writes_each_segment_with_the_options_it_maps_it_to() {
    let mut sink = MappedOptions::new(Recorder::default(), ByArrival::default());
    sink.sync_write_all_unpin(&b"abc"[..], Early(true)).unwrap();
    sink.sync_write_all_unpin(&b"d"[..], Early(false)).unwrap();
    let recorder = sink.into_parts().io;
    assert_eq!(
        recorder.segments,
        [
            segment(b"ab", Priority(0)),
            segment(b"c", Priority(0)),
            segment(b"d", Priority(9)),
        ]
    );
}

#[test]
fn an_options_mapping_maps_the_options_a_fill_returns() {
    let mut sink = MappedOptions::new(Recorder::default(), ByArrival::default());
    lend(&mut sink, b"ab", Some(Early(true)));
    lend(&mut sink, b"c", Some(Early(false)));
    lend(&mut sink, b"d", None::<Early>);
    let recorder = sink.into_parts().io;
    assert_eq!(
        recorder.segments,
        [segment(b"ab", Priority(0)), segment(b"c", Priority(9))]
    );
}

/// Counts its calls and lends one priority.
struct Counted {
    calls: usize,
    priority: Priority,
}
impl MapOptions<Early> for Counted {
    type Options = Priority;
    fn map_options<'a>(&'a mut self, _: &'a Early) -> &'a Priority {
        self.calls += 1;
        &self.priority
    }
}

#[test]
fn a_borrowed_mapping_retains_state_between_wrappers() {
    let mut recorder = Recorder::default();
    let mut mapping = Counted {
        calls: 0,
        priority: Priority(2),
    };
    for (index, bytes) in [b"ab".as_slice(), b"c".as_slice()].into_iter().enumerate() {
        let mut sink = MappedOptions::new(&mut recorder, &mut mapping);
        sink.sync_write_all_unpin(bytes, Early(true)).unwrap();
        assert_eq!(sink.map().calls, index + 1);
    }
    assert_eq!(mapping.calls, 2);
    assert_eq!(
        recorder.segments,
        [segment(b"ab", Priority(2)), segment(b"c", Priority(2))]
    );
}

#[test]
fn an_options_mapping_is_called_only_for_a_fill_that_returns_options() {
    let counted = Counted {
        calls: 0,
        priority: Priority(2),
    };
    let mut sink = MappedOptions::new(Recorder::default(), counted);
    assert!(matches!(
        lend(&mut sink, b"ab", Some(Early(true))),
        LentOutcome::Kept(_)
    ));
    assert!(matches!(
        lend(&mut sink, b"c", None::<Early>),
        LentOutcome::GaveUp(())
    ));
    let MappedOptionsParts {
        io: recorder,
        map: counted,
    } = sink.into_parts();
    assert_eq!(counted.calls, 1);
    assert_eq!(recorder.segments, [segment(b"ab", Priority(2))]);
}

#[test]
fn an_options_mapping_is_called_on_every_write_a_waiting_one_included() {
    let recorder = Recorder {
        pending: 1,
        ..Recorder::default()
    };
    let counted = Counted {
        calls: 0,
        priority: Priority(2),
    };
    let mut sink = MappedOptions::new(recorder, counted);
    let mut cx = Context::from_waker(Waker::noop());
    let mut input = &b"ab"[..];
    let waited = Pin::new(&mut sink).poll_write(&mut cx, &mut input, &Early(true));
    assert!(waited.is_pending());
    let written = Pin::new(&mut sink).poll_write(&mut cx, &mut input, &Early(true));
    assert!(
        matches!(written, Poll::Ready(Ok(Some(WriteSegment { count, .. }))) if count.get() == 2)
    );
    let MappedOptionsParts {
        io: recorder,
        map: counted,
    } = sink.into_parts();
    assert_eq!(counted.calls, 2);
    assert_eq!(recorder.segments, [segment(b"ab", Priority(2))]);
}

#[test]
fn fixed_options_write_every_segment_with_them_whatever_it_is_written_with() {
    let mut sink = MappedOptions::new(Recorder::default(), FixedOptions(Priority(3)));
    sink.sync_write_all_unpin(&b"ab"[..], ()).unwrap();
    sink.sync_write_all_unpin(&b"c"[..], Early(true)).unwrap();
    lend(&mut sink, b"d", Some(Early(false)));
    let recorder = sink.into_parts().io;
    assert_eq!(
        recorder.segments,
        [
            segment(b"ab", Priority(3)),
            segment(b"c", Priority(3)),
            segment(b"d", Priority(3)),
        ]
    );
}

struct NoClone(u8);

#[derive(Default)]
struct NonCloningSink {
    segments: Vec<(Bytes, u8)>,
}
family_receiver!(impl [] for NonCloningSink);
impl Write<NoClone> for NonCloningSink {
    type Error = !;
    type Report = ();
    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut B,
        options: &NoClone,
    ) -> Poll<Result<Option<WriteSegment<()>>, !>>
    where
        Self: 'v,
    {
        let Some(count) = NonZeroUsize::new(input.remaining().min(2)) else {
            return Poll::Ready(Ok(None));
        };
        let this = host.get_pin_mut().get_mut();
        this.segments
            .push((input.copy_to_bytes(count.get()), options.0));
        Poll::Ready(Ok(Some(WriteSegment { count, report: () })))
    }
}

#[test]
fn fixed_options_do_not_need_to_clone_options_for_ordinary_writes() {
    let mut sink = MappedOptions::new(NonCloningSink::default(), FixedOptions(NoClone(7)));
    sink.sync_write_all_unpin(&b"abc"[..], ()).unwrap();
    let sink = sink.into_parts().io;
    assert_eq!(
        sink.segments,
        [
            (Bytes::from_static(b"ab"), 7),
            (Bytes::from_static(b"c"), 7)
        ]
    );
}

/// Copy a source without metadata into a sink that takes priorities, all
/// of it with one.
#[call(sync)]
async fn copy_with<S, F>(
    source: &mut impl ReceiverAccess<Family = S>,
    sink: &mut impl ReceiverAccess<Family = F>,
    priority: Priority,
) -> u64
where
    S: ReadChunk<Metadata = (), Error = !>,
    F: Write<Priority, Report = (), Error = !>,
{
    let mut sink = MappedOptions::new(&mut *sink, FixedOptions(priority));
    let CopyTotal { count, .. } = copy_chunks(source, &mut sink, nz(8)).await.unwrap();
    count
}

#[test]
fn fixed_options_let_a_copy_write_a_source_without_metadata_with_them() {
    let mut hosts = (Bytes::from_static(b"abc"), Recorder::default());
    assert_eq!(hosts.sync_copy_with_unpin(Priority(6)), 3);
    assert_eq!(
        hosts.1.segments,
        [segment(b"ab", Priority(6)), segment(b"c", Priority(6))]
    );
}

/// Write `bytes` through `io`, which takes the options `O`.
#[call(sync)]
async fn write_with<F, O>(
    io: &mut impl ReceiverAccess<Family = F>,
    bytes: &[u8],
    options: O,
) -> Result<(), WriteAllError<F::Error>>
where
    F: Write<O, Report = ()>,
{
    io.write_all(bytes, options).await
}

#[test]
fn fixed_options_compose_on_an_access_inside_an_operation() {
    #[call(sync)]
    async fn tagged<F: Write<Priority, Report = (), Error = !>>(
        io: &mut impl ReceiverAccess<Family = F>,
    ) {
        let mut fixed = MappedOptions::new(&mut *io, FixedOptions(Priority(8)));
        write_with(&mut fixed, b"ab", ()).await.unwrap();
        write_with(&mut fixed, b"c", Early(true)).await.unwrap();
    }
    let mut sink = Recorder::default();
    sink.sync_tagged_unpin();
    assert_eq!(
        sink.segments,
        [segment(b"ab", Priority(8)), segment(b"c", Priority(8))]
    );
}

/// A sink of fixed options takes any options.
#[test]
fn fixed_options_make_a_sink_of_any_options() {
    fn writes<F: Write<O>, O>() {}
    writes::<OptionsMapping<Recorder<Priority>, FixedOptions<Priority>>, ()>();
    writes::<OptionsMapping<Recorder<Priority>, FixedOptions<Priority>>, Early>();
    writes::<OptionsMapping<Recorder<Priority>, FixedOptions<Priority>>, Priority>();
}

#[test]
fn the_mapping_applies_when_polling_a_view_directly() {
    use core::pin::pin;
    use nitori_call::Receiver;

    let mut sink = MappedOptions::new(Recorder::default(), FixedOptions(Priority(3)));
    let mut input = b"xyz".as_slice();
    {
        let view = pin!(Pin::new(&mut sink).view());
        let written = <OptionsMapping<Recorder<Priority>, FixedOptions<Priority>> as Write<
            Early,
        >>::poll_write(
            view,
            &mut Context::from_waker(Waker::noop()),
            &mut input,
            &Early(true),
        );
        assert!(matches!(written, Poll::Ready(Ok(Some(segment))) if segment.count.get() == 2));
    }
    assert_eq!(input, b"z");
    assert_eq!(sink.into_parts().io.segments, [segment(b"xy", Priority(3))]);
}

#[test]
fn a_mapping_can_wrap_an_inner_view_directly() {
    use core::pin::pin;
    use nitori_call::Receiver;

    let mut recorder = Recorder::default();
    let mut mapping = Counted {
        calls: 0,
        priority: Priority(4),
    };
    let mut input = b"xyz".as_slice();
    let view = Pin::new(&mut recorder).view();
    let written = <OptionsMapping<Recorder<Priority>, Counted> as Write<Early>>::poll_write(
        pin!(MappedOptions::new(view, &mut mapping)),
        &mut Context::from_waker(Waker::noop()),
        &mut input,
        &Early(true),
    );
    assert!(matches!(written, Poll::Ready(Ok(Some(segment))) if segment.count.get() == 2));
    assert_eq!(input, b"z");
    assert_eq!(mapping.calls, 1);
    assert_eq!(recorder.segments, [segment(b"xy", Priority(4))]);
}

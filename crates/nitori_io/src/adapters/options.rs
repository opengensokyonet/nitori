//! Writing with the write options mapped into those of the sink, for
//! writing bytes with options a sink does not take, such as the metadata
//! they were read with.
use crate::{Filled, LentBuffer, Write, WriteLent, WriteLentResult, WriteResult};
use bytes::Buf;
use core::{
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{Compose, ReceiverFamily};

/// The write options of a sink for those a segment is written with.
///
/// The options are lent, so a mapping chooses among options it keeps, such
/// as one set for bytes that arrived in 0-RTT and one for the rest, or lends
/// a part of the options it is given; one that computes them keeps the
/// result and lends it. It is called on every write of a segment, a write
/// that waits included, and once for each fill of a lending sink that
/// returns [`Filled`], not for one that gives up.
pub trait MapOptions<M> {
    /// The sink's write options.
    type Options;
    /// The sink's options for a segment written with `options`.
    fn map_options<'a>(&'a mut self, options: &'a M) -> &'a Self::Options;
}

/// Every segment gets these options, whatever it is written with.
///
/// Composed as [`MappedOptions`] on a sink of [`Write<O>`](Write), it makes a
/// sink of [`Write<M>`](Write) for every `M`, `()` included.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixedOptions<O>(pub O);
impl<M, O> MapOptions<M> for FixedOptions<O> {
    type Options = O;
    fn map_options<'a>(&'a mut self, _: &'a M) -> &'a O {
        &self.0
    }
}

/// Composition state: each segment written through it is written to the
/// parent with the options the mapping lends for its own.
///
/// Composed on a sink of [`Write<O>`](Write), `io.compose(MappedOptions(map))`
/// is a sink of [`Write<M>`](Write) for a `map: MapOptions<M, Options = O>`.
/// A copy whose source has metadata `M` can then write to a sink that takes
/// other options, chosen segment by segment, and
/// `MappedOptions(FixedOptions(o))` writes every segment with `o`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MappedOptions<P>(pub P);

/// The family of a sink composed with [`MappedOptions`] on a view of `F`.
///
/// Only [`Write`] and [`WriteLent`] are available through the composed
/// access, for every options type `M` the mapping maps into one of `F`.
pub struct OptionsMapping<F, P>(PhantomData<fn(F) -> F>, PhantomData<P>);
pin_project_lite::pin_project! {
    /// The view of an [`OptionsMapping`] family: the parent view, held by
    /// value, and the mapping.
    pub struct OptionsMappingView<'v, F, P>
    where
        F: ReceiverFamily,
        F: 'v,
    {
        #[pin]
        parent: F::ReceiverView<'v>,
        map: &'v mut P,
    }
}
impl<F: ReceiverFamily, P> fmt::Debug for OptionsMappingView<'_, F, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OptionsMappingView").finish_non_exhaustive()
    }
}
impl<F: ReceiverFamily, P> ReceiverFamily for OptionsMapping<F, P> {
    type ReceiverView<'v>
        = OptionsMappingView<'v, F, P>
    where
        Self: 'v;
}
impl<F: ReceiverFamily, P: Unpin> Compose<F> for MappedOptions<P> {
    type Family = OptionsMapping<F, P>;
    fn compose<'v>(
        self: Pin<&'v mut Self>,
        parent: F::ReceiverView<'v>,
    ) -> OptionsMappingView<'v, F, P>
    where
        F: 'v,
        Self::Family: 'v,
    {
        OptionsMappingView {
            parent,
            map: &mut self.get_mut().0,
        }
    }
}

/// Writes each segment to the parent with the options the mapping lends for
/// its own.
impl<F, P, M> Write<M> for OptionsMapping<F, P>
where
    P: MapOptions<M>,
    F: Write<P::Options>,
{
    type Error = F::Error;
    type Report = F::Report;
    fn poll_write<'v, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        input: &mut I,
        options: &M,
    ) -> Poll<WriteResult<Self, M>>
    where
        Self: 'v,
    {
        let view = host.project();
        let options = view.map.map_options(options);
        F::poll_write(view.parent, cx, input, options)
    }
}

/// Lends the parent's storage, and gives the parent a clone of the options
/// the mapping lends for the fill's when the fill returns.
impl<F, P, M> WriteLent<M> for OptionsMapping<F, P>
where
    P: MapOptions<M, Options: Clone>,
    F: WriteLent<P::Options>,
{
    type Buffer<'l> = F::Buffer<'l>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<WriteLentResult<Self, M, T, A>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<Self::Buffer<'l>>) -> Result<Filled<T, M>, A>,
    {
        let view = host.project();
        let map = view.map;
        F::poll_write_lent(view.parent, cx, wanted, |lent| {
            let Filled { value, options } = fill(lent)?;
            Ok(Filled {
                value,
                options: map.map_options(&options).clone(),
            })
        })
    }
}

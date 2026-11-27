//! Writing with the write options mapped into those of the sink, for
//! writing bytes with options a sink does not take, such as the metadata
//! they were read with.
use crate::{Filled, LentBuffer, Write, WriteLent, WriteLentResult};
use core::{
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{
    AccessPlan, Lent, ProvideAccess, Receiver, ReceiverAccess, ReceiverFamily, ViewProjection,
    project_access,
};

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
/// Used with [`MappedOptions`] on a sink of [`Write<O>`](Write), it makes a
/// sink of [`Write<M>`](Write) for every `M`, `()` included.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixedOptions<O>(pub O);
impl<M, O> MapOptions<M> for FixedOptions<O> {
    type Options = O;
    fn map_options<'a>(&'a mut self, _: &'a M) -> &'a O {
        &self.0
    }
}

pin_project_lite::pin_project! {
    /// A sink that writes with the options selected by its owned mapping.
    ///
    /// A `P: MapOptions<M, Options = O>` adapts a sink of [`Write<O>`](Write)
    /// to one of [`Write<M>`](Write). Use [`FixedOptions`] to give every segment
    /// the same options. On a lending sink the mapping runs when the fill succeeds.
    pub struct MappedOptions<IO, P> {
        #[pin]
        io: IO,
        map: P,
    }
}

impl<IO, P> MappedOptions<IO, P> {
    /// Wrap `io` with the given options mapping.
    pub const fn new(io: IO, map: P) -> Self {
        Self { io, map }
    }
    /// Borrow the inner IO and the mapping.
    pub const fn get_ref(&self) -> (&IO, &P) {
        (&self.io, &self.map)
    }
    /// Return the inner IO and the mapping.
    pub fn into_parts(self) -> (IO, P) {
        (self.io, self.map)
    }
}

impl<IO, P> fmt::Debug for MappedOptions<IO, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MappedOptions").finish_non_exhaustive()
    }
}

impl<IO, P> MappedOptions<IO, &mut P> {
    fn poll_write_with<I: ?Sized, M, R>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &mut I,
        options: &M,
        write: impl FnOnce(Pin<&mut IO>, &mut Context<'_>, &mut I, &P::Options) -> R,
    ) -> R
    where
        P: MapOptions<M>,
    {
        let this = self.project();
        let options = this.map.map_options(options);
        write(this.io, cx, input, options)
    }
}

/// The family of a sink of `F` adapted by an options mapping `P`.
pub struct OptionsMapping<F, P>(PhantomData<fn(F) -> F>, PhantomData<P>);

impl<F: ReceiverFamily, P> ReceiverFamily for OptionsMapping<F, P> {
    type ReceiverView<'v>
        = MappedOptions<F::ReceiverView<'v>, &'v mut P>
    where
        Self: 'v;
}

struct MapView<P>(PhantomData<P>);

impl<F: ReceiverFamily, P> ViewProjection<F> for MapView<P> {
    type State = P;
    type Family = OptionsMapping<F, P>;
    fn project<'v>(
        view: F::ReceiverView<'v>,
        state: &'v mut P,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v,
    {
        MappedOptions::new(view, state)
    }
}

impl<R: Receiver, P> Receiver for MappedOptions<R, P> {
    type Family = OptionsMapping<R::Family, P>;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        Self::Family: 'v,
    {
        let this = self.project();
        <MapView<P> as ViewProjection<R::Family>>::project(this.io.view(), this.map)
    }
}

impl<A: ReceiverAccess, P> ProvideAccess for MappedOptions<A, P> {
    type AccessFamily = OptionsMapping<Lent<A::Family>, P>;
    type Lentness = <A as ProvideAccess>::Lentness;
    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        project_access::<_, MapView<P>>(&mut self.io, &mut self.map)
    }
}

forward!(Write<M> for [F, P, M] OptionsMapping<F, P>, inner F, options <P as MapOptions<M>>::Options, with poll_write_with,
    where [P: MapOptions<M>, F: Write<<P as MapOptions<M>>::Options>]);

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
        let this = host.project();
        let map = this.map;
        F::poll_write_lent(this.io, cx, wanted, |lent| {
            let Filled { value, options } = fill(lent)?;
            Ok(Filled {
                value,
                options: map.map_options(&options).clone(),
            })
        })
    }
}

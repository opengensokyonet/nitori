//! Calls whose caller lends every view on every poll.
//!
//! An access of such a call has its view at hand, so [`LentAccess::with`] is
//! a plain synchronous call, and `.await` and `yield` are the only points
//! where other work can run. The closure cannot wait, and its views stay
//! inside it:
//!
//! ```compile_fail,E0728
//! use nitori_call::{Direct, LentAccess};
//! async fn step(io: &mut impl LentAccess<Family = Direct<u32>>) {
//!     io.with(|_| async {}.await);
//! }
//! ```
//!
//! ```compile_fail
//! use nitori_call::{Direct, LentAccess};
//! fn step(io: &mut impl LentAccess<Family = Direct<u32>>) {
//!     let _kept = io.with(|access| access.into_pin());
//! }
//! ```
//!
//! ```compile_fail,E0500
//! use nitori_call::{Direct, LentAccess};
//! fn step(io: &mut impl LentAccess<Family = Direct<u32>>) {
//!     io.with(|_| io.with(|_| ()));
//! }
//! ```
//!
//! Only a lent access has `with`:
//!
//! ```compile_fail,E0599
//! use nitori_call::{Direct, LentAccess, ReceiverAccess};
//! fn step(io: &mut impl ReceiverAccess<Family = Direct<u32>>) {
//!     io.with(|_| ());
//! }
//! ```
//!
//! An operation written for a lent access cannot run on a plain call, and a
//! lent call cannot be polled without a view of every receiver:
//!
//! ```compile_fail,E0277
//! #![feature(impl_trait_in_assoc_type)]
//! use nitori_call::{Call, Direct, LentAccess, ReceiverFamily, call};
//! #[call]
//! async fn needs_views<F: ReceiverFamily>(io: &mut impl LentAccess<Family = F>) {}
//! fn make() -> impl Sized {
//!     Call::<Direct<u32>, _>::new(NeedsViews::new())
//! }
//! ```
//!
//! ```compile_fail,E0277
//! #![feature(impl_trait_in_assoc_type)]
//! use nitori_call::{Direct, LentAccess, LentCall, call};
//! use std::{pin::pin, task::{Context, Waker}};
//! #[call]
//! async fn needs_views<F: nitori_call::ReceiverFamily>(io: &mut impl LentAccess<Family = F>) {}
//! fn poll() {
//!     let mut call = pin!(LentCall::<Direct<u32>, _>::new(NeedsViews::new()));
//!     let _ = call.as_mut().poll_with_views(None, &mut Context::from_waker(Waker::noop()));
//! }
//! ```
//!
//! Every receiver means every one of a tuple:
//!
//! ```compile_fail,E0277
//! #![feature(impl_trait_in_assoc_type)]
//! use nitori_call::{Direct, DirectView, LentCall, Receiver, ReceiverFamily};
//! use std::{pin::{Pin, pin}, task::{Context, Waker}};
//! fn poll<'v, Op>(
//!     call: Pin<&mut LentCall<(Direct<u8>, Direct<u8>), Op>>,
//!     view: Pin<&mut DirectView<'v, u8>>,
//! ) where
//!     Op: nitori_call::Operation<(Direct<u8>, Direct<u8>)>
//!         + for<'s> nitori_call::Start<'s, (nitori_call::LentSlot<'s, Direct<u8>>, nitori_call::LentSlot<'s, Direct<u8>>)>,
//! {
//!     let _ = call.poll_with_views((view, None::<()>), &mut Context::from_waker(Waker::noop()));
//! }
//! ```
//!
//! A lent operation has no host methods, so `sync` is an error:
//!
//! ```compile_fail
//! #![feature(impl_trait_in_assoc_type)]
//! use nitori_call::{LentAccess, ReceiverFamily, call};
//! #[call(sync)]
//! async fn hosted<F: ReceiverFamily>(io: &mut impl LentAccess<Family = F>) {}
//! ```
//!
//! and its method exists on lent accesses only:
//!
//! ```compile_fail,E0599
//! #![feature(impl_trait_in_assoc_type)]
//! use nitori_call::{LentAccess, ReceiverAccess, ReceiverFamily, call};
//! #[call]
//! async fn lent_only<F: ReceiverFamily>(io: &mut impl LentAccess<Family = F>) {}
//! fn plain<F: ReceiverFamily>(io: &mut impl ReceiverAccess<Family = F>) {
//!     let _ = io.lent_only();
//! }
//! ```
//!
//! A method of a capability whose result borrows the view is a method of the
//! view alone, and a method taking a view needs a lent access:
//!
//! ```compile_fail,E0599
//! # use nitori_call::{LentAccess, ReceiverFamily, capability};
//! # use std::pin::Pin;
//! #[capability]
//! pub trait Peek: ReceiverFamily {
//!     fn peek<'s, 'v>(view: Pin<&'s mut Self::ReceiverView<'v>>) -> &'s u8
//!     where
//!         Self: 'v,
//!         'v: 's;
//! }
//! fn peek(io: &mut impl LentAccess<Family: Peek>) {
//!     let _ = io.peek();
//! }
//! ```
//!
//! ```compile_fail,E0277
//! # use nitori_call::{ReceiverAccess, ReceiverFamily, capability};
//! # use std::pin::Pin;
//! #[capability]
//! pub trait Count: ReceiverFamily {
//!     fn count<'v>(view: Pin<&mut Self::ReceiverView<'v>>) -> u8
//!     where
//!         Self: 'v;
//! }
//! fn count(io: &mut impl ReceiverAccess<Family: Count>) -> u8 {
//!     io.count()
//! }
//! ```
use crate::{
    Access, Call, Emitter, Operation, ReceiverAccess, ReceiverFamily, ReceiverSlot, Receivers,
    Start, ViewOperation,
    access::{ReleasePoint, Sealed, ViewRequest, ViewVisitor},
};
use receivers::{LentReceivers, LentViews};
use std::{
    fmt,
    marker::PhantomData,
    ops::ControlFlow,
    pin::Pin,
    task::{Context, Poll},
};

/// An access whose call is lent a view on every poll, so that using the view
/// neither waits nor can end the poll. Other work runs only at `.await` and
/// `yield`; the code between them is atomic with respect to it.
///
/// The slots of a [`LentCall`], compositions of them and `&mut` borrows of
/// them are the only such accesses. Every operation written for a
/// [`ReceiverAccess`] works on them too.
pub trait LentAccess: ReceiverAccess {
    /// Run `body` once with this poll's view.
    ///
    /// # Panics
    /// Outside a poll, which only an operation's destructor can reach: use
    /// [`try_with`](Self::try_with) there. Like every use of a view, on a
    /// thread other than the one polling the call.
    #[track_caller]
    fn with<R, B>(&mut self, body: B) -> R
    where
        Self: Sized,
        B: for<'a, 'h> FnOnce(Access<'a, 'h, Self::Family>) -> R,
    {
        self.try_with(body)
            .expect("a lent view used outside a poll")
    }

    /// Run `body` once with this poll's view, or return `None` outside a
    /// poll, which only an operation's destructor can reach.
    ///
    /// # Panics
    /// Like every use of a view, on a thread other than the one polling the
    /// call.
    fn try_with<R, B>(&mut self, body: B) -> Option<R>
    where
        Self: Sized,
        B: for<'a, 'h> FnOnce(Access<'a, 'h, Self::Family>) -> R,
    {
        self.visit_view(WithView {
            body,
            output: PhantomData,
        })
    }
}
impl<A: LentAccess + ?Sized> LentAccess for &mut A {}

/// The visitor of [`LentAccess::try_with`].
struct WithView<B, R> {
    body: B,
    output: PhantomData<fn() -> R>,
}
impl<F: ReceiverFamily, B, R> ViewOperation<F> for WithView<B, R>
where
    B: for<'a, 'h> FnOnce(Access<'a, 'h, F>) -> R,
{
    type Output = Option<R>;
    #[inline]
    fn run<'h>(self, view: Pin<&mut F::ReceiverView<'h>>) -> Option<R>
    where
        F: 'h,
    {
        Some((self.body)(Access::new(view)))
    }
}
impl<F: ReceiverFamily, B, R> ViewVisitor<F> for WithView<B, R>
where
    B: for<'a, 'h> FnOnce(Access<'a, 'h, F>) -> R,
{
    fn absent(self, _: ViewRequest<'_>) -> Option<R> {
        None
    }
}

/// The slot a [`LentCall`] gives its operation: the call cannot be polled
/// without a view, so it is a [`LentAccess`].
pub struct LentSlot<'s, F: ReceiverFamily>(ReceiverSlot<'s, F>);

impl<F: ReceiverFamily> fmt::Debug for LentSlot<'_, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LentSlot").finish_non_exhaustive()
    }
}
impl<F: ReceiverFamily> Sealed for LentSlot<'_, F> {}
#[allow(private_bounds)]
impl<F: ReceiverFamily> ReceiverAccess for LentSlot<'_, F> {
    type Family = F;
    #[inline]
    fn visit_view<V: ViewVisitor<F>>(&mut self, visitor: V) -> V::Output {
        self.0.visit_view(visitor)
    }
    fn release_point(&mut self) -> ReleasePoint<'_> {
        self.0.release_point()
    }
}
impl<F: ReceiverFamily> LentAccess for LentSlot<'_, F> {}

/// Several lent accesses used at once, in one synchronous step: the closure
/// gets one [`Access`] for each, cannot wait, and keeps no view.
///
/// Implemented for tuples of 2 to 4 [`LentAccess`]es, such as
/// `(&mut a, &mut b)`.
pub trait LentAccesses<Body, Out> {
    /// Run `body` once with this poll's views.
    ///
    /// # Panics
    /// As for [`LentAccess::with`].
    fn with(self, body: Body) -> Out;
    /// Run `body` once with this poll's views, or return `None` outside a
    /// poll.
    ///
    /// # Panics
    /// As for [`LentAccess::try_with`].
    fn try_with(self, body: Body) -> Option<Out>;
}

/// Nests one `with` in another for each access, then calls the body.
macro_rules! with_views {
    ($method:ident, $body:expr, [$($done:ident)*]) => {
        $body($($done),*)
    };
    (with, $body:expr, [$($done:ident)*] $first:ident $($rest:ident)*) => {
        LentAccess::with(&mut $first, |$first| with_views!(with, $body, [$($done)* $first] $($rest)*))
    };
    (try_with, $body:expr, [$($done:ident)*] $first:ident $($rest:ident)*) => {
        LentAccess::try_with(&mut $first, |$first| with_views!(try_with, $body, [$($done)* $first] $($rest)*))
            .flatten()
    };
}

macro_rules! lent_tuple {
    ($($name:ident $view:ident $access:ident $index:tt $borrow:lifetime $inner:lifetime),+) => {
        impl<$($name: LentReceivers),+> LentReceivers for ($($name,)+) {
            type LentSlots<'s> = ($($name::LentSlots<'s>,)+);
            #[inline]
            fn lent<'s>(slots: Self::Slots<'s>) -> Self::LentSlots<'s> {
                ($($name::lent(slots.$index),)+)
            }
        }
        impl<'a, $($name: LentReceivers + 'a,)+ $($view: LentViews<'a, $name>),+>
            LentViews<'a, ($($name,)+)> for ($($view,)+)
        {
            #[inline]
            fn lends(self) -> <($($name,)+) as Receivers>::Lends<'a>
            where
                ($($name,)+): 'a,
            {
                ($(self.$index.lends(),)+)
            }
        }
        impl<$($name: LentAccess,)+ Body, Out> LentAccesses<Body, Out> for ($($name,)+)
        where
            Body: for<$($borrow, $inner),+> FnOnce($(Access<$borrow, $inner, $name::Family>),+) -> Out,
        {
            #[inline]
            #[track_caller]
            fn with(self, body: Body) -> Out {
                let ($(mut $access,)+) = self;
                with_views!(with, body, [] $($access)+)
            }
            #[inline]
            fn try_with(self, body: Body) -> Option<Out> {
                let ($(mut $access,)+) = self;
                with_views!(try_with, (|$($access),+| Some(body($($access),+))), [] $($access)+)
            }
        }
    };
}
lent_tuple!(A V a 0 'a0 'h0, B W b 1 'a1 'h1);
lent_tuple!(A V a 0 'a0 'h0, B W b 1 'a1 'h1, C X c 2 'a2 'h2);
lent_tuple!(A V a 0 'a0 'h0, B W b 1 'a1 'h1, C X c 2 'a2 'h2, D Y d 3 'a3 'h3);

mod receivers {
    //! What a `LentCall` needs of its receivers and of its views. Unnameable
    //! outside this crate: a family, or a tuple of them.
    use super::LentSlot;
    use crate::{Accesses, Lend, ReceiverFamily, Receivers, ViewLoan};
    use std::pin::Pin;

    /// Receivers lent on every poll.
    pub trait LentReceivers: Receivers {
        /// The slots of the operation of a `LentCall`.
        type LentSlots<'s>: Accesses<Families = Self>;
        fn lent<'s>(slots: Self::Slots<'s>) -> Self::LentSlots<'s>;
    }
    impl<F: ReceiverFamily> LentReceivers for F {
        type LentSlots<'s> = LentSlot<'s, F>;
        #[inline]
        fn lent<'s>(slot: Self::Slots<'s>) -> LentSlot<'s, F> {
            LentSlot(slot)
        }
    }

    /// What a caller lends for one poll: for a family, its pinned view or a
    /// loan of one, and for a tuple, one of these for each receiver.
    #[diagnostic::on_unimplemented(
        message = "`{Self}` does not lend a view of every receiver of this call",
        label = "a lent call takes a pinned view of each receiver, in the receivers' shape"
    )]
    pub trait LentViews<'a, R: Receivers> {
        fn lends(self) -> R::Lends<'a>
        where
            R: 'a;
    }
    impl<'a, 'view: 'a, F: ReceiverFamily + 'view> LentViews<'a, F>
        for Pin<&'a mut F::ReceiverView<'view>>
    {
        #[inline]
        fn lends(self) -> Option<Lend<ViewLoan<'a, F>>>
        where
            F: 'a,
        {
            Some(Lend::Fixed(ViewLoan::new(self)))
        }
    }
    impl<'a, F: ReceiverFamily> LentViews<'a, F> for ViewLoan<'a, F> {
        #[inline]
        fn lends(self) -> Option<Lend<ViewLoan<'a, F>>>
        where
            F: 'a,
        {
            Some(Lend::Fixed(self))
        }
    }
}

/// Turns an operation on lent slots into one on the slots of a [`Call`].
struct LentOperation<R, Op>(Op, PhantomData<fn(R)>);

impl<R: Receivers, Op: Operation<R>> Operation<R> for LentOperation<R, Op> {
    type Output = Op::Output;
    type Yield = Op::Yield;
}
impl<'s, R, Op> Start<'s, R::Slots<'s>> for LentOperation<R, Op>
where
    R: LentReceivers,
    Op: Operation<R> + Start<'s, R::LentSlots<'s>>,
{
    type Fut = <Op as Start<'s, R::LentSlots<'s>>>::Fut;
    #[inline]
    fn start(
        self,
        slots: R::Slots<'s>,
        emitter: Emitter<'s, <Op as Operation<R>>::Yield>,
    ) -> Self::Fut {
        self.0.start(R::lent(slots), emitter)
    }
}

type Output<R, Op> = <Op as Operation<R>>::Output;
type Yield<R, Op> = <Op as Operation<R>>::Yield;

/// A [`Call`] whose caller lends a view of every receiver on every poll.
///
/// The operation sees [`LentSlot`]s, which are [`LentAccess`]es. Its slots
/// cannot ask for a view, and a fixed view has no release points to offer, so
/// polling gives a plain [`Poll`]. Like a call, it is stored apart from its
/// resources: the views may come from elsewhere on each poll.
///
/// A family's view is lent as a `Pin<&mut _>`, and a tuple of receivers as a
/// tuple of them. A [`ViewLoan`](crate::ViewLoan) can stand in for any of them.
pub struct LentCall<R, Op>
where
    R: LentReceivers,
    Op: Operation<R> + for<'s> Start<'s, R::LentSlots<'s>>,
{
    call: Call<R, LentOperation<R, Op>>,
}
impl<R, Op> fmt::Debug for LentCall<R, Op>
where
    R: LentReceivers,
    Op: Operation<R> + for<'s> Start<'s, R::LentSlots<'s>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LentCall")
            .field("terminated", &self.is_terminated())
            .finish_non_exhaustive()
    }
}
impl<R, Op> LentCall<R, Op>
where
    R: LentReceivers,
    Op: Operation<R> + for<'s> Start<'s, R::LentSlots<'s>>,
{
    /// Prepare a call. The operation starts on the first poll.
    pub fn new(operation: Op) -> Self {
        Self {
            call: Call::new(LentOperation(operation, PhantomData)),
        }
    }

    /// Whether the call has delivered its final value and must not be polled
    /// again.
    pub fn is_terminated(&self) -> bool {
        self.call.is_terminated()
    }

    fn call(self: Pin<&mut Self>) -> Pin<&mut Call<R, LentOperation<R, Op>>> {
        // SAFETY: the call is structurally pinned and never moved out.
        unsafe { self.map_unchecked_mut(|this| &mut this.call) }
    }

    /// Advance the operation with a view of every receiver. `Continue` holds
    /// an emitted item and `Break` the final value, after which the call is
    /// done.
    #[inline]
    pub fn poll_with_views<'a, V: LentViews<'a, R>>(
        self: Pin<&mut Self>,
        views: V,
        cx: &mut Context<'_>,
    ) -> Poll<ControlFlow<Output<R, Op>, Yield<R, Op>>>
    where
        R: 'a,
    {
        self.call().poll_with_views(views.lends(), cx).lent()
    }

    /// Advance to the final value with a view of every receiver. Emitted
    /// items are dropped where they are emitted, without suspending the
    /// operation.
    #[inline]
    pub fn poll_return_with_views<'a, V: LentViews<'a, R>>(
        self: Pin<&mut Self>,
        views: V,
        cx: &mut Context<'_>,
    ) -> Poll<Output<R, Op>>
    where
        R: 'a,
    {
        self.call().poll_return_with_views(views.lends(), cx).lent()
    }
}

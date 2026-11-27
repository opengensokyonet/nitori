//! Telling an access where the values decoded from it and encoded into it
//! start and end.
use crate::{
    AccessPlan, ReceiverAccess,
    access::{PlanSealed, ReleasePoint, ViewVisitor},
};

/// A type that tracks the values decoded from and encoded into the access
/// it provides: where each starts, and whether it was finished or abandoned.
///
/// The decoding and encoding of `nitori_codec` report each value: the
/// start before its first byte, then exactly one of finished, when the value
/// was decoded or encoded, or abandoned, when it failed or its future was
/// dropped. A value made of others reports theirs between its start and its
/// end, so an abandonment between two of its fields is reported for the
/// outer value too. Bytes read or written outside a value are not reported.
///
/// Attach the tracker to the plan the type issues with [`track_values`].
/// Whoever forwards that plan, such as a wrapper that stores the type and
/// issues its plan, forwards the reports with it. Every method does nothing
/// by default. A value abandoned before it transfers a byte still reports
/// abandonment. The tracker decides what that event means for the source
/// or sink.
pub trait TrackValues {
    /// A value started decoding.
    fn decode_started(&mut self) {}
    /// The value that started decoding last and has not ended was decoded.
    fn decode_finished(&mut self) {}
    /// The value that started decoding last and has not ended was abandoned:
    /// its decoding failed or its future was dropped.
    fn decode_abandoned(&mut self) {}
    /// A value started encoding.
    fn encode_started(&mut self) {}
    /// The value that started encoding last and has not ended was encoded.
    fn encode_finished(&mut self) {}
    /// The value that started encoding last and has not ended was
    /// abandoned: its encoding failed or its future was dropped.
    fn encode_abandoned(&mut self) {}
}

/// Where a value is, as reported to [`TrackValues`].
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueEvent {
    DecodeStarted,
    DecodeFinished,
    DecodeAbandoned,
    EncodeStarted,
    EncodeFinished,
    EncodeAbandoned,
}

impl ValueEvent {
    pub(crate) fn tell(self, tracker: &mut (impl TrackValues + ?Sized)) {
        match self {
            Self::DecodeStarted => tracker.decode_started(),
            Self::DecodeFinished => tracker.decode_finished(),
            Self::DecodeAbandoned => tracker.decode_abandoned(),
            Self::EncodeStarted => tracker.encode_started(),
            Self::EncodeFinished => tracker.encode_finished(),
            Self::EncodeAbandoned => tracker.encode_abandoned(),
        }
    }
}

/// Report `event` to the trackers along the plans of `access`.
pub(crate) fn report<A: ReceiverAccess + ?Sized>(access: &mut A, event: ValueEvent) {
    access.access_plan().track(event);
}

/// Issue `plan` with `tracker` attached: the values decoded from and
/// encoded into the access through this plan are reported to `tracker`,
/// then to the trackers of the plan's own parents.
///
/// ```
/// use nitori_call::{AccessPlan, ProvideAccess, ReceiverAccess, TrackValues, track_values};
///
/// /// Counts the values that were abandoned.
/// #[derive(Default)]
/// struct Abandoned(usize);
/// impl TrackValues for Abandoned {
///     fn decode_abandoned(&mut self) {
///         self.0 += 1;
///     }
/// }
///
/// struct Payload<A> {
///     io: A,
///     abandoned: Abandoned,
/// }
///
/// impl<A: ReceiverAccess> ProvideAccess for Payload<A> {
///     type AccessFamily = A::Family;
///     type Lentness = A::Lentness;
///
///     fn access_plan<'a>(
///         &'a mut self,
///     ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
///         track_values(self.io.access_plan(), &mut self.abandoned)
///     }
/// }
/// ```
pub fn track_values<'a, P, T>(
    plan: P,
    tracker: &'a mut T,
) -> impl AccessPlan<'a, Family = P::Family, Lentness = P::Lentness>
where
    P: AccessPlan<'a>,
    T: TrackValues + ?Sized,
{
    Tracked { plan, tracker }
}

/// A plan with a tracker attached.
struct Tracked<'a, P, T: ?Sized> {
    plan: P,
    tracker: &'a mut T,
}

impl<'a, P: AccessPlan<'a>, T: TrackValues + ?Sized> PlanSealed for Tracked<'a, P, T> {}

impl<'a, P: AccessPlan<'a>, T: TrackValues + ?Sized> AccessPlan<'a> for Tracked<'a, P, T> {
    type Family = P::Family;
    type Lentness = P::Lentness;

    fn visit<V: ViewVisitor<Self::Family>>(self, visitor: V) -> V::Output {
        self.plan.visit(visitor)
    }

    fn release(self) -> ReleasePoint<'a> {
        self.plan.release()
    }

    fn track(self, event: ValueEvent) {
        event.tell(self.tracker);
        self.plan.track(event);
    }
}

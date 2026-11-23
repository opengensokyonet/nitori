//! A panic: through a poll, or through `Driver::new`, `add`, `remove` or
//! `end`.
use super::*;

/// A part with a call on the timer that panics, and a session component
/// whose end panics once armed.
pub struct Boomer;
empty_share!(Boomer: Peer, Path, Route);
impl Share<Connection> for Boomer {
    type Data = HNil;
}
impl ShareCalls<Connection> for Boomer {
    type Calls = HList![StartBoom];
}
thread_local! {
    static NEW_ARMED: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}
pub struct StartBoom;
impl<Q: Composition> CallKind<Q> for StartBoom {
    type Needs = HNil;
    type Op = StartBooming;
    fn start() -> StartBooming {
        if NEW_ARMED.with(|a| a.replace(false)) {
            panic!("start boom");
        }
        StartBooming::new()
    }
}
#[call]
pub async fn start_booming<W: CallFamily<C = StartBoom>>(host: &mut impl LentAccess<Family = W>) {
    let _ = host;
    core::future::pending::<()>().await;
}
impl Share<Session> for Boomer {
    type Data = HList![Fuse];
}
impl ShareCalls<Session> for Boomer {
    type Calls = HNil;
}
thread_local! {
    static ARMED: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
    static BUILD_ARMED: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}
#[derive(Debug, Default)]
pub struct Fuse;
impl<L: PartList<L> + 'static> Hooks<Boomer, L, Session> for Fuse {
    fn build<KP: KeyedList<L, BuildKeyed<Self, Boomer, L, Session>>>(
        _: BuildView<'_, Self, Boomer, L, Session, KP>,
        _: &ArgsOf<L>,
        _: &mut (),
    ) -> Self {
        if BUILD_ARMED.with(|a| a.replace(false)) {
            panic!("fuse build");
        }
        Fuse
    }
    fn end<KP: HookKeys<Self, Boomer, L, Session>>(
        _: HookView<'_, Self, Boomer, L, Session, KP>,
        _: nitori_driver::hooks::EndCause<'_, EndReasonOf<Session>>,
    ) {
        if ARMED.with(|a| a.replace(false)) {
            panic!("fuse");
        }
    }
}
impl Share<Timer> for Boomer {
    type Data = HNil;
}
impl ShareCalls<Timer> for Boomer {
    type Calls = HList![Boom];
}
impl<L> nitori_driver::hooks::Part<L> for Boomer {
    type Args = ();
}
pub struct Boom;
impl<Q: Composition> CallKind<Q> for Boom {
    type Needs = HNil;
    type Op = Booming;
    fn start() -> Booming {
        Booming::new()
    }
}
#[call]
pub async fn booming<W: CallFamily<C = Boom>>(host: &mut impl LentAccess<Family = W>) {
    let _ = host;
    panic!("boom");
}
pub struct QB;
composition!(impl driver for QB: Connection, HList![Base, Watch, Boomer], TestClock);

#[test]
fn a_panic_tells_the_root_only() {
    log();
    let mut d =
        Driver::<QB>::new(TestClock::default(), frunk_core::hlist![(), (), ()], ()).unwrap();
    d.add::<Peer, _, _, _>(End, PeerId(1), ()).unwrap();
    let p = d.add::<Path, _, _, _>(End, PathId(1), ()).unwrap();
    let r = d.add::<Route, _, _, _>(p, PeerId(1), ()).unwrap();
    d.add::<Timer, _, _, _>(r, (), ()).unwrap();
    log();
    let (_, w) = outer();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poll(&mut d, &w)));
    assert!(unwound.is_err());
    log();
    drop(d);
    assert_eq!(log(), ["aborted Connection Panicked", "remains"]);
}

/// The session removed with its fuse armed: its end hook panics.
fn a_remove_that_panics() -> Driver<QB> {
    log();
    let mut d =
        Driver::<QB>::new(TestClock::default(), frunk_core::hlist![(), (), ()], ()).unwrap();
    let s = d.add::<Session, _, _, _>(End, (), ()).unwrap();
    log();
    ARMED.with(|a| a.set(true));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.remove(s, ())));
    assert!(unwound.is_err());
    assert_eq!(log(), ["end Session"]);
    d
}

#[test]
fn a_panic_in_an_entry_is_one_at_the_drop() {
    let d = a_remove_that_panics();
    drop(d);
    // no end hook runs again, on a tree half changed
    assert_eq!(log(), ["aborted Connection Panicked", "remains"]);
}

#[test]
fn an_entry_after_a_panic_in_an_entry_does_nothing() {
    let mut d = a_remove_that_panics();
    assert!(d.add::<Path, _, _, _>(End, PathId(1), ()).is_err());
    assert_eq!(log(), ["aborted Connection Panicked", "remains"]);
    assert!(d.ended());
    drop(d);
    assert!(log().is_empty());
}

#[test]
fn add_that_panics_is_panicked_at_drop() {
    log();
    let mut d =
        Driver::<QB>::new(TestClock::default(), frunk_core::hlist![(), (), ()], ()).unwrap();
    log();
    BUILD_ARMED.with(|a| a.set(true));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        d.add::<Session, _, _, _>(End, (), ())
    }));
    assert!(unwound.is_err());
    let _ = log();
    drop(d);
    assert_eq!(log(), ["aborted Connection Panicked", "remains"]);
}

#[test]
fn end_that_panics_is_panicked_at_drop() {
    log();
    let mut d =
        Driver::<QB>::new(TestClock::default(), frunk_core::hlist![(), (), ()], ()).unwrap();
    d.add::<Session, _, _, _>(End, (), ()).unwrap();
    log();
    ARMED.with(|a| a.set(true));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.end(())));
    assert!(unwound.is_err());
    log();
    drop(d);
    assert_eq!(log(), ["aborted Connection Panicked", "remains"]);
}

#[test]
fn poll_after_entry_panic_is_ready() {
    let mut d = a_remove_that_panics();
    let (_, w) = outer();
    assert_eq!(poll(&mut d, &w), Poll::Ready(()));
    assert_eq!(log(), ["aborted Connection Panicked", "remains"]);
    assert!(d.ended());
    assert_eq!(poll(&mut d, &w), Poll::Ready(()));
    assert!(log().is_empty());
}

#[test]
fn remove_and_end_after_entry_panic() {
    log();
    let mut d =
        Driver::<QB>::new(TestClock::default(), frunk_core::hlist![(), (), ()], ()).unwrap();
    let s = d.add::<Session, _, _, _>(End, (), ()).unwrap();
    let p = d.add::<Path, _, _, _>(End, PathId(1), ()).unwrap();
    log();
    ARMED.with(|a| a.set(true));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.remove(s, ())));
    assert!(unwound.is_err());
    log();
    assert!(matches!(d.remove(p, ()), Err(RemoveError::NoNode)));
    assert_eq!(log(), ["aborted Connection Panicked", "remains"]);
    d.end(());
    assert!(log().is_empty());
    drop(d);
    assert!(log().is_empty());
}

#[test]
fn new_that_panics_is_panicked_at_drop() {
    log();
    NEW_ARMED.with(|a| a.set(true));
    let unwound = std::panic::catch_unwind(|| {
        Driver::<QB>::new(TestClock::default(), frunk_core::hlist![(), (), ()], ()).map(|_| ())
    });
    assert!(unwound.is_err());
    assert_eq!(log(), ["aborted Connection Panicked", "remains"]);
}

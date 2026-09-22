#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{
    BoundCall, HasReceiverFamily, Receiver, ReceiverFamily, Target, TargetExt, call,
};
use std::{
    future::{Future, poll_fn},
    pin::{Pin, pin},
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
struct Resource {
    builds: AtomicUsize,
    drops: AtomicUsize,
}
struct Family;
struct View<'a> {
    resource: &'a Resource,
    _not_send: Rc<()>,
}
impl Drop for View<'_> {
    fn drop(&mut self) {
        self.resource.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl HasReceiverFamily for View<'_> {
    type Family = Family;
}
impl ReceiverFamily for Family {
    type ReceiverView<'a> = View<'a>;
}
impl Receiver for Resource {
    type Family = Family;
    fn view<'a>(self: Pin<&'a mut Self>) -> View<'a>
    where
        Family: 'a,
    {
        let resource = self.into_ref().get_ref();
        resource.builds.fetch_add(1, Ordering::SeqCst);
        View {
            resource,
            _not_send: Rc::new(()),
        }
    }
}
#[call]
async fn use_twice(io: Target<Family>) -> usize {
    let mut first = true;
    poll_fn(|cx| {
        if std::mem::take(&mut first) {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    io.with(|_| ()).await;
    io.with(|_| ()).await;
    let mut first = true;
    poll_fn(|cx| {
        if std::mem::take(&mut first) {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    io.with(|_| 7).await
}
#[test]
fn non_send_view_does_not_prevent_moving_suspended_execution_between_threads() {
    let mut resource = Resource {
        builds: AtomicUsize::new(0),
        drops: AtomicUsize::new(0),
    };
    let mut execution = pin!(BoundCall::new(Pin::new(&mut resource), use_twice()));
    assert!(
        execution
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    std::thread::scope(|scope| {
        let mut execution = execution.as_mut();
        scope
            .spawn(move || {
                assert!(
                    execution
                        .as_mut()
                        .poll(&mut Context::from_waker(Waker::noop()))
                        .is_pending()
                );
            })
            .join()
            .unwrap();
    });
    assert_eq!(
        execution
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(7)
    );
}
#[test]
fn ordinary_wait_is_lazy_and_each_drive_reuses_and_releases_its_view() {
    let mut resource = Resource {
        builds: AtomicUsize::new(0),
        drops: AtomicUsize::new(0),
    };
    let mut call = pin!(use_twice());
    use nitori_call::PollCallExt;
    assert!(
        call.as_mut()
            .poll_receiver(
                Pin::new(&mut resource),
                &mut Context::from_waker(Waker::noop())
            )
            .is_pending()
    );
    assert_eq!(resource.builds.load(Ordering::SeqCst), 0);
    assert!(
        call.as_mut()
            .poll_receiver(
                Pin::new(&mut resource),
                &mut Context::from_waker(Waker::noop())
            )
            .is_pending()
    );
    assert_eq!(
        (
            resource.builds.load(Ordering::SeqCst),
            resource.drops.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
    assert!(
        call.as_mut()
            .poll_receiver(
                Pin::new(&mut resource),
                &mut Context::from_waker(Waker::noop())
            )
            .is_ready()
    );
    assert_eq!(
        (
            resource.builds.load(Ordering::SeqCst),
            resource.drops.load(Ordering::SeqCst)
        ),
        (2, 2)
    );
}

#[call(yields = O::Yield)]
async fn owned_driver<O: nitori_call::CallOn<Family>>(
    _control: Target<nitori_call::ExecutionControl>,
    resource: Resource,
    operation: O,
) -> O::Return {
    let resource = resource;
    let acquisition = nitori_call::AcquisitionState::<Family, _, _>::new(|| async {
        resource.builds.fetch_add(1, Ordering::SeqCst);
        View {
            resource: &resource,
            _not_send: Rc::new(()),
        }
    });
    let mut acquisition = pin!(acquisition);
    let mut operation = pin!(operation);
    loop {
        match nitori_call::drive(acquisition.as_mut(), operation.as_mut()).await {
            std::ops::CoroutineState::Yielded(value) => yield value,
            std::ops::CoroutineState::Complete(value) => return value,
        }
    }
}
#[test]
fn captured_driver_execution_is_send_even_when_its_view_is_not() {
    let resource = Resource {
        builds: AtomicUsize::new(0),
        drops: AtomicUsize::new(0),
    };
    let mut execution = pin!(nitori_call::Execution::new(owned_driver(
        resource,
        use_twice()
    )));
    assert!(
        execution
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    assert!(
        execution
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    std::thread::scope(|scope| {
        let mut execution = execution.as_mut();
        scope
            .spawn(move || {
                assert_eq!(
                    execution
                        .as_mut()
                        .poll(&mut Context::from_waker(Waker::noop())),
                    Poll::Ready(7)
                )
            })
            .join()
            .unwrap();
    });
}

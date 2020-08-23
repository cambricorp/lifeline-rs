use futures::{task::AtomicWaker, Future};
use std::fmt::Debug;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::Poll,
};

use crate::error::type_name;
use log::debug;
use pin_project::pin_project;

/// Executes the task, until the future completes, or the lifeline is dropped
pub(crate) fn spawn_task<O>(name: String, fut: impl Future<Output = O> + Send + 'static) -> Lifeline
where
    O: Debug + Send + 'static,
{
    let inner = Arc::new(LifelineInner::new());

    let service = LifelineFuture::new(name, fut, inner.clone());
    spawn_task_inner(service);

    Lifeline::new(inner)
}

pub(crate) fn task_name<S>(name: &str) -> String {
    type_name::<S>().to_string() + "/" + name
}

// #[cfg(feature = "tokio-executor")]
fn spawn_task_inner<F, O>(task: F)
where
    F: Future<Output = O> + Send + 'static,
    O: Send + 'static,
{
    tokio::spawn(task);
}

#[pin_project]
struct LifelineFuture<F: Future> {
    #[pin]
    future: F,
    name: String,
    inner: Arc<LifelineInner>,
}

impl<F: Future + Send> LifelineFuture<F> {
    pub fn new(name: String, future: F, inner: Arc<LifelineInner>) -> Self {
        debug!("START {}", &name);

        Self {
            name,
            future,
            inner,
        }
    }
}

impl<F: Future> Future for LifelineFuture<F>
where
    F::Output: Debug,
{
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        if self.inner.cancel.load(Ordering::Relaxed) {
            debug!("CANCEL {}", self.name);
            return Poll::Ready(());
        }

        // attempt to complete the future
        if let Poll::Ready(result) = self.as_mut().project().future.poll(cx) {
            debug!("END {} {:?}", self.name, result);
            return Poll::Ready(());
        }

        // Register to receive a wakeup if the future is aborted in the... future
        self.inner.waker.register(cx.waker());

        // Check to see if the future was aborted between the first check and
        // registration.
        // Checking with `Relaxed` is sufficient because `register` introduces an
        // `AcqRel` barrier.
        if self.inner.cancel.load(Ordering::Relaxed) {
            debug!("CANCEL {}", self.name);
            return Poll::Ready(());
        }

        Poll::Pending
    }
}

#[derive(Debug)]
#[must_use = "if unused the service will immediately be cancelled"]
pub struct Lifeline {
    inner: Arc<LifelineInner>,
}

impl Lifeline {
    pub(crate) fn new(inner: Arc<LifelineInner>) -> Self {
        Self { inner }
    }
}

impl Drop for Lifeline {
    fn drop(&mut self) {
        self.inner.abort();
    }
}

#[derive(Debug)]
pub(crate) struct LifelineInner {
    waker: AtomicWaker,
    cancel: AtomicBool,
}

impl LifelineInner {
    pub fn new() -> Self {
        LifelineInner {
            waker: AtomicWaker::new(),
            cancel: AtomicBool::new(false),
        }
    }

    pub fn abort(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.waker.wake();
    }
}
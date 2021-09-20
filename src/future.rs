use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use pin_project_lite::pin_project;

use crate::{CreateFuture, RetryPolicy};

macro_rules! try_retry {
    ($retry:expr, $result:expr) => {
        match $retry.retry(Some($result.as_ref())) {
            Some(retry_fut) => FutureState::WaitingRetry { retry_fut },
            None => return Poll::Ready($result),
        }
    };
}

macro_rules! ready {
    ($e:expr $(,)?) => {
        match $e {
            Poll::Ready(t) => t,
            Poll::Pending => return Poll::Pending,
        }
    };
}

pin_project! {
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct ConcurrentRetry<R, T, E, F, CF>
    where
        R: RetryPolicy<T, E>,
    {
        create_f: CF,
        #[pin]
        running_futs: Futs<F>,
        #[pin]
        future_state: FutureState<R, T, E>
    }
}

impl<R, T, E, F, CF> ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
{
    pub(crate) fn new(retry: R, create_f: CF) -> Self {
        Self {
            running_futs: Futs::empty(),
            create_f,
            future_state: FutureState::CreateFut { retry: Some(retry) },
        }
    }
}

impl<R, T, E, F, CF> Future for ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
    F: Future<Output = Result<T, E>>,
    CF: CreateFuture<F>,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let this = self.as_mut().project();
            let new_state = match this.future_state.project() {
                FutureStateProj::CreateFut { retry } => {
                    this.running_futs.push(this.create_f.create());
                    FutureState::PollResults {
                        retry: retry.take(),
                    }
                }
                FutureStateProj::WaitingDelay { delay, retry } => {
                    match this.running_futs.poll(cx) {
                        Poll::Ready(result) => {
                            let retry = retry.take().expect("Waiting retry must be some");
                            try_retry!(retry, result)
                        }
                        Poll::Pending => {
                            ready!(delay.poll(cx));
                            let retry = retry.take().expect("Waiting retry must be some");
                            match retry.retry(None) {
                                Some(retry_fut) => FutureState::WaitingRetry { retry_fut },
                                None => FutureState::PollResults { retry: None },
                            }
                        }
                    }
                }
                FutureStateProj::WaitingRetry { retry_fut } => {
                    let retry = ready!(retry_fut.poll(cx));
                    FutureState::CreateFut { retry: Some(retry) }
                }
                FutureStateProj::PollResults { retry } => match retry.take() {
                    Some(retry) => match this.running_futs.poll(cx) {
                        Poll::Ready(result) => try_retry!(retry, result),
                        Poll::Pending => FutureState::WaitingDelay {
                            delay: retry.force_retry_after(),
                            retry: Some(retry),
                        },
                    },
                    None => return this.running_futs.poll(cx),
                },
            };
            self.as_mut().project().future_state.set(new_state);
        }
    }
}

pin_project! {
    #[project = FutureStateProj]
    enum FutureState<R, T, E>
    where
        R: RetryPolicy<T, E>,
        {
            CreateFut { retry: Option<R> },
            WaitingDelay { #[pin] delay: R::ForceRetryFuture, retry: Option<R> },
            WaitingRetry { #[pin] retry_fut: R::RetryFuture },
            PollResults { retry: Option<R> },
    }
}

pin_project! {
    struct Futs<F> {
        #[pin]
        fut: Option<F>,
        rest: Vec<Pin<Box<F>>>,
    }
}

impl<F> Futs<F> {
    fn empty() -> Self {
        Self {
            fut: None,
            rest: vec![],
        }
    }

    fn push(self: Pin<&mut Self>, f: F) {
        let mut this = self.project();
        if this.fut.is_none() {
            this.fut.set(Some(f))
        } else {
            this.rest.push(Box::pin(f))
        }
    }
}

impl<F, T, E> Futs<F>
where
    F: Future<Output = Result<T, E>>,
{
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let mut this = self.project();
        match this.fut.as_mut().as_pin_mut().map(|x| x.poll(cx)) {
            Some(Poll::Ready(r)) => {
                this.fut.set(None);
                Poll::Ready(r)
            }
            Some(Poll::Pending) => poll_vec(this.rest, cx),
            None => {
                if this.rest.is_empty() {
                    panic!("Futs are empty. This is a bug")
                }
                poll_vec(this.rest, cx)
            }
        }
    }
}

fn poll_vec<F: Future + Unpin>(v: &mut Vec<F>, cx: &mut Context<'_>) -> Poll<F::Output> {
    v.iter_mut()
        .enumerate()
        .find_map(|(i, f)| match Pin::new(f).poll(cx) {
            Poll::Pending => None,
            Poll::Ready(e) => Some((i, e)),
        })
        .map(|(idx, r)| {
            v.swap_remove(idx);
            Poll::Ready(r)
        })
        .unwrap_or(Poll::Pending)
}

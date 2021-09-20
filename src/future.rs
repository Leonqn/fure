use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use pin_project_lite::pin_project;

use crate::{CreateFuture, Policy};

macro_rules! try_policy {
    ($policy:expr, $result:expr) => {
        match $policy.retry(Some($result.as_ref())) {
            Some(policy_fut) => RetryState::WaitingRetry { policy_fut },
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
    pub struct Retry<P, T, E, F, CF>
    where
        P: Policy<T, E>,
    {
        create_f: CF,
        #[pin]
        running_futs: Futs<F>,
        #[pin]
        retry_state: RetryState<P, T, E>
    }
}

impl<P, T, E, F, CF> Retry<P, T, E, F, CF>
where
    P: Policy<T, E>,
{
    pub(crate) fn new(policy: P, create_f: CF) -> Self {
        Self {
            running_futs: Futs::empty(),
            create_f,
            retry_state: RetryState::CreateFut {
                policy: Some(policy),
            },
        }
    }
}

impl<P, T, E, F, CF> Future for Retry<P, T, E, F, CF>
where
    P: Policy<T, E>,
    F: Future<Output = Result<T, E>>,
    CF: CreateFuture<F>,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let this = self.as_mut().project();
            let new_state = match this.retry_state.project() {
                RetryStateProj::CreateFut { policy } => {
                    this.running_futs.push(this.create_f.create());
                    RetryState::PollResults {
                        policy: policy.take(),
                    }
                }
                RetryStateProj::WaitingDelay { delay, policy } => {
                    match this.running_futs.poll(cx) {
                        Poll::Ready(result) => {
                            let policy = policy.take().expect("Waiting policy must be some");
                            try_policy!(policy, result)
                        }
                        Poll::Pending => {
                            ready!(delay.poll(cx));
                            let policy = policy.take().expect("Waiting policy must be some");
                            match policy.retry(None) {
                                Some(policy_fut) => RetryState::WaitingRetry { policy_fut },
                                None => RetryState::PollResults { policy: None },
                            }
                        }
                    }
                }
                RetryStateProj::WaitingRetry { policy_fut } => {
                    let policy = ready!(policy_fut.poll(cx));
                    RetryState::CreateFut {
                        policy: Some(policy),
                    }
                }
                RetryStateProj::PollResults { policy } => match policy.take() {
                    Some(policy) => match this.running_futs.poll(cx) {
                        Poll::Ready(result) => try_policy!(policy, result),
                        Poll::Pending => RetryState::WaitingDelay {
                            delay: policy.force_retry_after(),
                            policy: Some(policy),
                        },
                    },
                    None => return this.running_futs.poll(cx),
                },
            };
            self.as_mut().project().retry_state.set(new_state);
        }
    }
}

pin_project! {
    #[project = RetryStateProj]
    enum RetryState<P, T, E>
    where
        P: Policy<T, E>,
        {
            CreateFut { policy: Option<P> },
            WaitingDelay { #[pin] delay: P::ForceRetryFuture, policy: Option<P> },
            WaitingRetry { #[pin] policy_fut: P::RetryFuture },
            PollResults { policy: Option<P> },
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

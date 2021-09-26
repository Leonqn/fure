use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use pin_project_lite::pin_project;

mod concurrent;
mod sequential;

pub use concurrent::*;
pub use sequential::*;

use crate::Policy;

pub trait PolicyExt: Sized {
    fn attempts(self, max_retries: usize) -> RetryAttempts<Self, usize>;
    fn retry_if<T, E, FN>(self, retry_if: FN) -> RetryAttempts<Self, FN>
    where
        FN: FnMut(Option<Result<&T, &E>>) -> bool;
}

impl<P> PolicyExt for P {
    fn attempts(self, max_retries: usize) -> RetryAttempts<P, usize> {
        RetryAttempts {
            policy: self,
            condition: max_retries,
        }
    }

    fn retry_if<T, E, FN>(self, retry_if: FN) -> RetryAttempts<P, FN>
    where
        FN: FnMut(Option<Result<&T, &E>>) -> bool,
    {
        RetryAttempts {
            policy: self,
            condition: retry_if,
        }
    }
}

pub struct RetryAttempts<P, C> {
    policy: P,
    condition: C,
}

impl<P, T, E, FN> Policy<T, E> for RetryAttempts<P, FN>
where
    P: Policy<T, E>,
    FN: FnMut(Option<Result<&T, &E>>) -> bool,
{
    type ForceRetryFuture = P::ForceRetryFuture;

    type RetryFuture = RetryMap<P::RetryFuture, FN>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        self.policy.force_retry_after()
    }

    fn retry(mut self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        if (self.condition)(result) {
            None
        } else {
            Some(RetryMap {
                policy_f: self.policy.retry(result)?,
                add_field: Some(self.condition),
            })
        }
    }
}

impl<P, T, E> Policy<T, E> for RetryAttempts<P, usize>
where
    P: Policy<T, E>,
{
    type ForceRetryFuture = P::ForceRetryFuture;

    type RetryFuture = RetryMap<P::RetryFuture, usize>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        self.policy.force_retry_after()
    }

    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        match result {
            Some(Ok(_)) => None,
            _ => self.condition.checked_sub(1).and_then(|a| {
                Some(RetryMap {
                    policy_f: self.policy.retry(result)?,
                    add_field: Some(a),
                })
            }),
        }
    }
}

pin_project! {
    pub struct RetryMap<F, C>
    {
        #[pin]
        policy_f: F,
        add_field: Option<C>,
    }
}

impl<P, F, C> Future for RetryMap<F, C>
where
    F: Future<Output = P>,
{
    type Output = RetryAttempts<P, C>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.policy_f.poll(cx) {
            Poll::Ready(policy) => Poll::Ready(RetryAttempts {
                policy,
                condition: this
                    .add_field
                    .take()
                    .expect("RetryMap add_field must be some"),
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

use crate::Policy;
use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

mod concurrent;
mod sequential;

pub use concurrent::*;
pub use sequential::*;

/// Creates a policy to retry futures specified number of times
pub fn attempts<P>(policy: P, max_retries: usize) -> RetryAttempts<P, usize> {
    RetryAttempts {
        policy,
        condition: max_retries,
    }
}

/// Returns a policy to retry futures while `retry_if` returns `true`
pub fn retry_if<P, T, E, FN>(policy: P, retry_if: FN) -> RetryAttempts<P, FN>
where
    FN: FnMut(Option<Result<&T, &E>>) -> bool,
{
    RetryAttempts {
        policy,
        condition: retry_if,
    }
}

/// An policy created by [`attempts`] and [`retry_if`].
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
            Some(RetryMap {
                policy_f: self.policy.retry(result)?,
                add_field: Some(self.condition),
            })
        } else {
            None
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

#[cfg(test)]
mod tests {

    mod attempts {
        use std::sync::{Arc, Mutex};

        use crate::{
            policies::{attempts, sequential},
            retry,
            tests::run_test,
        };

        #[test]
        fn should_retry_specified_number_of_times() {
            run_test(async move {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || async {
                    crate::tests::yield_now().await;
                    let mut call_count = call_count.lock().unwrap();
                    *call_count += 1;
                    Err::<(), ()>(())
                };
                let policy = sequential();
                let retry_if = attempts(policy, 2);

                let result = retry(create_fut, retry_if).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 3);
                assert!(result.is_err());
            })
        }

        #[test]
        fn should_not_retry_ok_result() {
            run_test(async move {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || async {
                    crate::tests::yield_now().await;
                    let mut call_count = call_count.lock().unwrap();
                    *call_count += 1;
                    Ok::<(), ()>(())
                };
                let policy = sequential();
                let retry_if = attempts(policy, 2);

                let result = retry(create_fut, retry_if).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 1);
                assert!(result.is_ok());
            })
        }
    }

    mod retry_if {
        use std::sync::{Arc, Mutex};

        use crate::{
            policies::{retry_if, sequential},
            retry,
            tests::run_test,
        };

        #[test]
        fn should_retry_if_returns_true() {
            run_test(async move {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || async {
                    crate::tests::yield_now().await;
                    let mut call_count = call_count.lock().unwrap();
                    *call_count += 1;
                    Err::<(), ()>(())
                };
                let policy = sequential();
                let mut tries_left = 3;
                let retry_if = retry_if(policy, |result| {
                    tries_left -= 1;
                    tries_left != 0 && !matches!(result, Some(Ok(_)))
                });

                let result = retry(create_fut, retry_if).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 3);
                assert!(result.is_err());
            })
        }
    }
}

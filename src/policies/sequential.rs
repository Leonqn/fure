use std::{
    future::{pending, ready, Future, Pending, Ready},
    pin::Pin,
    task::{Context, Poll},
};

use crate::Policy;

use super::AttemptsPolicy;

pub trait SequentialPolicy<T, E>: Sized {
    type RetryFuture: Future<Output = Self>;

    fn retry(self, result: Result<&T, &E>) -> Option<Self::RetryFuture>;
}

#[cfg(any(feature = "tokio", feature = "async-std"))]
mod retry_backoff {
    use super::*;
    use std::{marker::PhantomData, time::Duration};

    pub struct BackoffPolicy<'a, P, I> {
        policy: P,
        backoff: I,
        _phantom: PhantomData<&'a ()>,
    }

    impl<'a, P, I> BackoffPolicy<'a, P, I> {
        pub fn new(policy: P, backoff: I) -> Self {
            Self {
                policy,
                backoff,
                _phantom: Default::default(),
            }
        }
    }

    impl<'a, P, I, T, E> SequentialPolicy<T, E> for BackoffPolicy<'a, P, I>
    where
        P: SequentialPolicy<T, E> + Send + 'a,
        P::RetryFuture: Send + 'a,
        I: Iterator<Item = Duration> + Send + 'a,
    {
        type RetryFuture = Pin<Box<dyn Future<Output = Self> + Send + 'a>>;

        fn retry(self, result: Result<&T, &E>) -> Option<Self::RetryFuture> {
            let policy = self.policy.retry(result)?;
            let mut backoff = self.backoff;
            let delay = backoff.next();
            let policy = Box::pin(async move {
                let new_policy = policy.await;
                if let Some(delay) = delay {
                    #[cfg(feature = "tokio")]
                    tokio::time::sleep(delay).await;
                    #[cfg(feature = "async-std")]
                    async_std::task::sleep(delay).await;
                }

                Self::new(new_policy, backoff)
            });
            Some(policy)
        }
    }
}
pub use retry_backoff::*;

pub struct SequentialRetry<P> {
    policy: P,
}

impl<P> SequentialRetry<P> {
    pub fn new(policy: P) -> Self {
        Self { policy }
    }
}

impl<P, T, E> Policy<T, E> for SequentialRetry<P>
where
    P: SequentialPolicy<T, E>,
{
    type ForceRetryFuture = Pending<()>;
    type RetryFuture = SeqMap<P, T, E>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        pending()
    }

    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        let result = result.expect("Result must be some");
        let policy_f = self.policy.retry(result)?;
        Some(SeqMap { policy_f })
    }
}

impl<T, E> SequentialPolicy<T, E> for AttemptsPolicy {
    type RetryFuture = Ready<Self>;

    fn retry(self, result: Result<&T, &E>) -> Option<Self::RetryFuture> {
        self.retry(Some(result)).map(ready)
    }
}

pin_project_lite::pin_project! {
    pub struct SeqMap<P, T, E>
    where
        P: SequentialPolicy<T, E>,
    {
        #[pin]
        policy_f: P::RetryFuture,
    }
}

impl<P, T, E> Future for SeqMap<P, T, E>
where
    P: SequentialPolicy<T, E>,
{
    type Output = SequentialRetry<P>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().policy_f.poll(cx) {
            Poll::Ready(policy) => Poll::Ready(SequentialRetry { policy }),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::{
        policies::{sequential::SequentialRetry, AttemptsPolicy},
        retry,
    };

    #[cfg(any(feature = "tokio", feature = "async-std"))]
    mod retry_backoff {
        use std::time::{Duration, Instant};

        use crate::{backoff, policies::sequential::BackoffPolicy};

        use super::*;

        #[tokio::test]
        async fn should_run_next_after_backoff() {
            let create_fut = || async move {
                tokio::task::yield_now().await;
                Err::<(), ()>(())
            };
            let now = Instant::now();

            let _result = tokio::spawn(retry(
                create_fut,
                SequentialRetry::new(BackoffPolicy::new(
                    AttemptsPolicy::new(2),
                    backoff::exponential(
                        Duration::from_millis(50),
                        2,
                        Some(Duration::from_secs(1)),
                    ),
                )),
            ))
            .await;

            assert!(now.elapsed() > Duration::from_millis(150))
        }
    }

    mod retry_failed {
        use super::*;

        #[tokio::test]
        async fn should_run_futures_sequentially() {
            let call_count = Arc::new(Mutex::new(0));
            let pass_allowed = Arc::new(Mutex::new(false));
            let create_fut = {
                let call_count = call_count.clone();
                let pass_allowed = pass_allowed.clone();
                move || {
                    let call_count = call_count.clone();
                    let pass_allowed = pass_allowed.clone();
                    async move {
                        {
                            let mut mutex_guard = call_count.lock().unwrap();
                            *mutex_guard += 1;
                        }
                        loop {
                            {
                                if *pass_allowed.lock().unwrap() {
                                    break;
                                }
                            }
                            tokio::task::yield_now().await
                        }

                        Err::<(), ()>(())
                    }
                }
            };

            tokio::spawn(async move {
                retry(create_fut, SequentialRetry::new(AttemptsPolicy::new(1))).await
            });
            tokio::task::yield_now().await;

            {
                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 1);
            }
            *pass_allowed.lock().unwrap() = true;
            tokio::task::yield_now().await;
            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 2);
        }
    }
}

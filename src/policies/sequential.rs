use std::{
    future::{pending, ready, Future, Pending, Ready},
    pin::Pin,
    task::{Context, Poll},
};

use super::Attempts;
use crate::Policy;
use pin_project_lite::pin_project;

/// Creates a policy to run futures sequentially.
/// ## Example
/// Sends at most 4 requests and returns the first [`Ok`] result.
/// ```
/// # async fn run() -> Result<(), reqwest::Error> {
/// use fure::policies::{sequential, Attempts};
///
/// let get_body = || async {
///     reqwest::get("https://www.rust-lang.org")
///         .await?
///         .text()
///         .await
/// };
/// let body = fure::retry(get_body, sequential(Attempts::new(3))).await?;
/// println!("body = {}", body);
/// # Ok(())
/// # }
/// ```
pub fn sequential<T, E>(policy: Attempts<T, E>) -> SequentialRetryPolicy<T, E> {
    SequentialRetryPolicy { policy }
}

/// A policy is created by [`sequential`] function
pub struct SequentialRetryPolicy<T, E> {
    policy: Attempts<T, E>,
}

impl<T, E> Policy<T, E> for SequentialRetryPolicy<T, E> {
    type ForceRetryFuture = Pending<()>;
    type RetryFuture = Ready<Self>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        pending()
    }

    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        let policy = self.policy.retry(result)?;
        Some(ready(Self { policy }))
    }
}

#[cfg(any(feature = "tokio", feature = "async-std"))]
mod retry_backoff {
    use super::*;
    use std::time::Duration;

    /// Creates a policy to run futures sequentially with specified backoff.
    /// ## Example
    /// Sends at most 4 requests and returns the first [`Ok`] result.
    ///
    /// Each next request will be sent only after specified backoff.
    /// ```
    /// # async fn run() -> Result<(), reqwest::Error> {
    /// use fure::{backoff::fixed, policies::{backoff, Attempts}};
    /// use std::time::Duration;
    ///
    /// let get_body = || async {
    ///     reqwest::get("https://www.rust-lang.org")
    ///         .await?
    ///         .text()
    ///         .await
    /// };
    /// let fixed = fixed(Duration::from_secs(3));
    /// let body = fure::retry(get_body, backoff(Attempts::new(3), fixed)).await?;
    /// println!("body = {}", body);
    /// # Ok(())
    /// # }
    /// ```
    pub fn backoff<T, E, I>(policy: Attempts<T, E>, backoff: I) -> BackoffRetry<T, E, I> {
        BackoffRetry { policy, backoff }
    }

    /// A policy is created by [`backoff`] function
    pub struct BackoffRetry<T, E, I> {
        policy: Attempts<T, E>,
        backoff: I,
    }

    impl<I, T, E> Policy<T, E> for BackoffRetry<T, E, I>
    where
        I: Iterator<Item = Duration>,
    {
        type ForceRetryFuture = Pending<()>;
        type RetryFuture = SeqDelay<T, E, I>;

        fn force_retry_after(&self) -> Self::ForceRetryFuture {
            pending()
        }

        fn retry(mut self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
            let policy = self.policy.retry(result)?;
            let delay = self.backoff.next().map(crate::sleep::sleep);
            Some(SeqDelay {
                backoff: Some(self.backoff),
                delay,
                result: Some(policy),
            })
        }
    }

    pin_project! {
        /// A future for [`backoff`] policy
        pub struct SeqDelay<T, E, I>
        {
            backoff: Option<I>,
            #[pin]
            delay: Option<crate::sleep::Sleep>,
            result: Option<Attempts<T, E>>
        }
    }

    impl<T, E, I> Future for SeqDelay<T, E, I>
    where
        I: Iterator<Item = Duration>,
    {
        type Output = BackoffRetry<T, E, I>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.project();
            match this.delay.as_pin_mut().map(|x| x.poll(cx)) {
                Some(Poll::Pending) => Poll::Pending,
                _ => Poll::Ready(backoff(
                    this.result.take().expect("SeqDelay result must be some"),
                    this.backoff.take().expect("SeqDelay Backoff must be some"),
                )),
            }
        }
    }
}
#[cfg(any(feature = "tokio", feature = "async-std"))]
pub use retry_backoff::*;

#[cfg(all(any(feature = "tokio", feature = "async-std"), test))]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tests::run_test;
    use crate::{policies::Attempts, retry};

    mod retry_backoff {
        use std::time::{Duration, Instant};

        use crate::{backoff::exponential, policies::sequential::backoff};

        use super::*;

        #[test]
        fn should_run_next_after_backoff() {
            run_test(async {
                let create_fut = || async move {
                    crate::tests::yield_now().await;
                    Err::<(), ()>(())
                };
                let now = Instant::now();

                let _result = crate::tests::spawn(retry(
                    create_fut,
                    backoff(
                        Attempts::new(2),
                        exponential(Duration::from_millis(50), 2, Some(Duration::from_secs(1))),
                    ),
                ))
                .await;

                assert!(now.elapsed() > Duration::from_millis(150))
            })
        }
    }

    mod retry_failed {
        use std::time::Duration;

        use crate::policies::sequential::sequential;

        use super::*;

        #[test]
        fn should_run_futures_sequentially() {
            run_test(async {
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
                                crate::tests::yield_now().await
                            }

                            Err::<(), ()>(())
                        }
                    }
                };

                crate::tests::spawn(async move {
                    retry(create_fut, sequential(Attempts::new(1))).await
                });
                crate::sleep::sleep(Duration::from_millis(10)).await;

                {
                    let guard = call_count.lock().unwrap();
                    assert_eq!(*guard, 1);
                }
                *pass_allowed.lock().unwrap() = true;
                crate::sleep::sleep(Duration::from_millis(10)).await;
                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
            })
        }
    }
}

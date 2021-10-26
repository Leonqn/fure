use std::future::{pending, ready, Pending, Ready};

use crate::Policy;

/// Creates a policy to run futures sequentially.
///
/// Note: this policy has no stop condition, so for getting a result you should wrap it with [attempts](`super::attempts`), [cond](`super::cond`) or your own wrapper.
/// ## Example
/// Sends at most 4 requests and returns the first [`Ok`] result.
/// ```
/// # async fn run() -> Result<(), reqwest::Error> {
/// use fure::policies::{sequential, attempts};
///
/// let get_body = || async {
///     reqwest::get("https://www.rust-lang.org")
///         .await?
///         .text()
///         .await
/// };
/// let body = fure::retry(get_body, attempts(sequential(), 3)).await?;
/// println!("body = {}", body);
/// # Ok(())
/// # }
/// ```
pub fn sequential() -> SequentialRetryPolicy {
    SequentialRetryPolicy
}

/// A policy is created by [`sequential`] function
#[derive(Debug, Clone, Copy)]
pub struct SequentialRetryPolicy;

impl<T, E> Policy<T, E> for SequentialRetryPolicy {
    type ForceRetryFuture = Pending<()>;
    type RetryFuture = Ready<Self>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        pending()
    }

    fn retry(self, _result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        Some(ready(self))
    }
}

#[cfg(any(feature = "tokio", feature = "async-std"))]
mod retry_backoff {
    use super::*;
    use pin_project_lite::pin_project;
    use std::time::Duration;
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    };

    /// Creates a policy to run futures sequentially with specified backoff.
    ///
    /// Note: this policy has no stop condition, so for getting a result you should wrap it with [attempts](`super::super::attempts`), [cond](`super::super::cond`) or your own wrapper.
    /// ## Example
    /// Sends at most 4 requests and returns the first [`Ok`] result.
    ///
    /// Each next request will be sent only after specified backoff.
    /// ```
    /// # async fn run() -> Result<(), reqwest::Error> {
    /// use fure::{backoff::fixed, policies::{backoff, attempts}};
    /// use std::time::Duration;
    ///
    /// let get_body = || async {
    ///     reqwest::get("https://www.rust-lang.org")
    ///         .await?
    ///         .text()
    ///         .await
    /// };
    /// let fixed = fixed(Duration::from_secs(3));
    /// let body = fure::retry(get_body, attempts(backoff(fixed), 3)).await?;
    /// println!("body = {}", body);
    /// # Ok(())
    /// # }
    /// ```
    pub fn backoff<I>(backoff: I) -> BackoffRetry<I> {
        BackoffRetry { backoff }
    }

    /// A policy is created by [`backoff`] function
    #[derive(Debug, Clone, Copy)]
    pub struct BackoffRetry<I> {
        backoff: I,
    }

    impl<I, T, E> Policy<T, E> for BackoffRetry<I>
    where
        I: Iterator<Item = Duration>,
    {
        type ForceRetryFuture = Pending<()>;
        type RetryFuture = SeqDelay<I>;

        fn force_retry_after(&self) -> Self::ForceRetryFuture {
            pending()
        }

        fn retry(mut self, _result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
            let delay = self.backoff.next().map(crate::sleep::sleep);
            Some(SeqDelay {
                backoff: Some(self.backoff),
                delay,
            })
        }
    }

    pin_project! {
        /// A future for [`backoff`] policy
        pub struct SeqDelay<I>
        {
            backoff: Option<I>,
            #[pin]
            delay: Option<crate::sleep::Sleep>,
        }
    }

    impl<I> Future for SeqDelay<I>
    where
        I: Iterator<Item = Duration>,
    {
        type Output = BackoffRetry<I>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.project();
            match this.delay.as_pin_mut().map(|x| x.poll(cx)) {
                Some(Poll::Pending) => Poll::Pending,
                _ => Poll::Ready(backoff(
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

    use crate::policies::attempts;
    use crate::retry;
    use crate::tests::run_test;
    mod retry_backoff {
        use std::time::{Duration, Instant};

        use crate::{backoff::exponential, policies::sequential::backoff};

        use super::*;

        #[test]
        fn should_run_next_after_backoff() {
            run_test(async {
                let create_fut = || async {
                    crate::tests::yield_now().await;
                    Err::<(), ()>(())
                };
                let now = Instant::now();

                let policy = backoff(exponential(
                    Duration::from_millis(50),
                    2,
                    Some(Duration::from_secs(1)),
                ));
                let result = retry(create_fut, attempts(policy, 2)).await;

                assert!(now.elapsed() > Duration::from_millis(150));
                assert!(result.is_err());
            })
        }
    }

    mod attempts {
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

                crate::tests::spawn(
                    async move { retry(create_fut, attempts(sequential(), 2)).await },
                );
                crate::sleep::sleep(Duration::from_millis(10)).await;

                {
                    let guard = call_count.lock().unwrap();
                    assert_eq!(*guard, 1);
                }
                *pass_allowed.lock().unwrap() = true;
                crate::sleep::sleep(Duration::from_millis(10)).await;
                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 3);
            })
        }
    }
}

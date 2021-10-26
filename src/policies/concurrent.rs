use std::future::{ready, Ready};

use crate::Policy;

/// Creates a policy to run futures concurrently.
///
/// If one of the futures completes immediately no next futures will be run.
///
/// Note: this policy has no stop condition, so for getting a result you should wrap it with [attempts](`super::attempts`), [cond](`super::cond`) or your own wrapper.
/// ## Example
/// Sends at most 4 concurrent requests and waits for one with an [`Ok`] result.
///
/// ```
/// # async fn run() -> Result<(), reqwest::Error> {
/// use fure::policies::{concurrent, attempts};
///
/// let get_body = || async {
///     reqwest::get("https://www.rust-lang.org")
///         .await?
///         .text()
///         .await
/// };
/// let body = fure::retry(get_body, attempts(concurrent(), 3)).await?;
/// println!("body = {}", body);
/// # Ok(())
/// # }
/// ```
pub fn concurrent() -> ConcurrentRetryPolicy {
    ConcurrentRetryPolicy
}

/// A policy is created by [`concurrent`] function.
#[derive(Debug, Clone, Copy)]
pub struct ConcurrentRetryPolicy;

impl<T, E> Policy<T, E> for ConcurrentRetryPolicy {
    type ForceRetryFuture = Ready<()>;
    type RetryFuture = Ready<Self>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        ready(())
    }

    fn retry(self, _result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        Some(ready(Self))
    }
}
#[cfg(any(feature = "tokio", feature = "async-std"))]
mod _interval {
    use super::*;
    use std::time::Duration;

    /// Creates a policy to run additional future after `force_retry_after` has passed without a result.
    ///
    /// After each completed future the previous delay is dropped and a new one is started.
    ///
    /// Note: this policy has no stop condition, so for getting a result you should wrap it with [attempts](`super::super::attempts`), [cond](`super::super::cond`) or your own wrapper.
    /// ## Example
    /// Sends at most 4 concurrent requests and waits for one with an [`Ok`] result.
    ///
    /// Every next future will be run only after 1 second.
    ///
    /// If request takes less than 1 second no next futures will be run.
    /// ```
    /// # async fn run() -> Result<(), reqwest::Error> {
    /// use fure::policies::{interval, attempts};
    /// use std::time::Duration;
    ///
    /// let get_body = || async {
    ///     reqwest::get("https://www.rust-lang.org")
    ///         .await?
    ///         .text()
    ///         .await
    /// };
    /// let body = fure::retry(get_body, attempts(interval(Duration::from_secs(1)), 3)).await?;
    /// println!("body = {}", body);
    /// # Ok(())
    /// # }
    /// ```
    pub fn interval(force_retry_after: Duration) -> IntervalRetryPolicy {
        IntervalRetryPolicy { force_retry_after }
    }

    /// A policy is created by [`interval`] function.
    #[derive(Debug, Clone, Copy)]
    pub struct IntervalRetryPolicy {
        force_retry_after: Duration,
    }

    impl<T, E> Policy<T, E> for IntervalRetryPolicy {
        type ForceRetryFuture = crate::sleep::Sleep;
        type RetryFuture = Ready<Self>;

        fn force_retry_after(&self) -> Self::ForceRetryFuture {
            crate::sleep::sleep(self.force_retry_after)
        }

        fn retry(self, _result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
            Some(ready(self))
        }
    }
}
#[cfg(any(feature = "tokio", feature = "async-std"))]
pub use _interval::*;

#[cfg(test)]
mod test {
    use std::sync::{Arc, Mutex};

    use crate::policies::attempts;
    use crate::retry;
    use crate::tests::run_test;
    use std::future::pending;

    mod concurrent {

        use crate::policies::concurrent::concurrent;

        use super::*;

        #[test]
        fn should_run_all_non_ready_futures() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || async {
                    let call = {
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        *mutex_guard
                    };

                    if call == 3 {
                        // we need 3 Poll::Pending to run one more future before returning an ok result
                        // create -> poll (1) -> waiting_delay (2)(it also polls) -> create -> poll (3)
                        crate::tests::yield_now().await;
                        crate::tests::yield_now().await;
                        crate::tests::yield_now().await;
                        Ok::<(), ()>(())
                    } else {
                        pending().await
                    }
                };

                let result = retry(create_fut, attempts(concurrent(), 4)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 4);
                assert!(result.is_ok());
            })
        }

        #[test]
        fn should_run_futures_till_ready_one() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || async {
                    let call = {
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        *mutex_guard
                    };
                    if call == 2 {
                        Ok::<(), ()>(())
                    } else {
                        pending().await
                    }
                };

                let result = retry(create_fut, attempts(concurrent(), 5)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
                assert!(result.is_ok());
            })
        }
    }

    #[cfg(any(feature = "tokio", feature = "async-std"))]
    mod delayed_concurrent {

        use std::time::{Duration, Instant};

        use crate::policies::concurrent::interval;

        use super::*;

        #[test]
        fn should_return_last_error_when_all_attempts() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || async {
                    crate::tests::yield_now().await;
                    let mut mutex_guard = call_count.lock().unwrap();
                    *mutex_guard += 1;
                    if *mutex_guard == 1 {
                        Err::<(), _>(*mutex_guard)
                    } else {
                        Err(*mutex_guard)
                    }
                };

                let result = retry(
                    create_fut,
                    attempts(interval(Duration::from_secs(10000)), 2),
                )
                .await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 3);
                assert_eq!(result, Err(3));
            })
        }

        #[test]
        fn should_retry_after_delay() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || async {
                    crate::tests::yield_now().await;
                    let call_count = {
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        *mutex_guard
                    };
                    if call_count == 1 {
                        pending::<Result<(), ()>>().await
                    } else {
                        Ok(())
                    }
                };
                let now = Instant::now();
                let result =
                    retry(create_fut, attempts(interval(Duration::from_millis(50)), 2)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
                assert!(now.elapsed() >= Duration::from_millis(50));
                assert!(result.is_ok());
            })
        }
    }
}

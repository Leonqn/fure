use std::future::{ready, Ready};

use crate::Policy;

/// Creates a policy to run futures concurrently.
///
/// If one of the futures completes immediately (without returning [`std::task::Poll::Pending`]) no next futures will be run.
/// ## Example
/// Sends at most 4 concurrent requests and waits for one with an [`Ok`] result.
///
/// ```
/// # async fn run() -> Result<(), reqwest::Error> {
/// use fure::policies::{parallel, Attempts};
///
/// let get_body = || async {
///     reqwest::get("https://www.rust-lang.org")
///         .await?
///         .text()
///         .await
/// };
/// let body = fure::retry(get_body, parallel(Attempts::new(3))).await?;
/// println!("body = {}", body);
/// # Ok(())
/// # }
/// ```
pub fn parallel() -> ParallelRetryPolicy {
    ParallelRetryPolicy
}

/// A policy is created by [`parallel`] function
#[derive(Debug, Clone, Copy)]
pub struct ParallelRetryPolicy;

impl<T, E> Policy<T, E> for ParallelRetryPolicy {
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
mod delayed {
    use super::*;
    use std::time::Duration;

    /// Creates a policy to run additional future in case of error result or `force_retry_after` time elapsed without getting a result.
    ///
    /// After each completed future the previous delay is dropped and a new one started.
    /// ## Example
    /// Sends at most 4 concurrent requests and waits for one with an [`Ok`] result.
    ///
    /// Every next future will be run only after 1 second.
    ///
    /// If request takes less than 1 second no next futures will be run.
    /// ```
    /// # async fn run() -> Result<(), reqwest::Error> {
    /// use fure::policies::{interval, Attempts};
    /// use std::time::Duration;
    ///
    /// let get_body = || async {
    ///     reqwest::get("https://www.rust-lang.org")
    ///         .await?
    ///         .text()
    ///         .await
    /// };
    /// let body = fure::retry(get_body, interval(Attempts::new(3), Duration::from_secs(1))).await?;
    /// println!("body = {}", body);
    /// # Ok(())
    /// # }
    /// ```
    pub fn interval(force_retry_after: Duration) -> IntervalRetryPolicy {
        IntervalRetryPolicy { force_retry_after }
    }

    /// A policy is created by [`interval`] function
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
pub use delayed::*;

#[cfg(test)]
mod test {
    use std::sync::{Arc, Mutex};

    use crate::retry;
    use crate::tests::run_test;
    use std::future::pending;

    mod concurrent {

        use crate::policies::{concurrent::parallel, PolicyExt};

        use super::*;

        #[test]
        fn should_run_only_one_future_when_first_completed() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        Ok::<(), ()>(())
                    }
                };

                let result = retry(create_fut, parallel().attempts(2)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 1);
                assert!(result.is_ok());
            })
        }

        #[test]
        fn should_run_all_non_ready_futures() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let call = {
                            let mut mutex_guard = call_count.lock().unwrap();
                            *mutex_guard += 1;
                            *mutex_guard
                        };
                        if call == 3 {
                            Ok::<(), ()>(())
                        } else {
                            pending().await
                        }
                    }
                };

                let result = retry(create_fut, parallel().attempts(2)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 3);
                assert!(result.is_ok());
            })
        }

        #[test]
        fn should_run_futures_till_ready_one() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
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
                    }
                };

                let result = retry(create_fut, parallel().attempts(2)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
                assert!(result.is_ok());
            })
        }
    }

    #[cfg(any(feature = "tokio", feature = "async-std"))]
    mod delayed_concurrent {

        use std::time::{Duration, Instant};

        use crate::policies::{concurrent::interval, PolicyExt};

        use super::*;

        #[test]
        fn should_retry_when_failed() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        if *mutex_guard == 1 {
                            Err(())
                        } else {
                            Ok(())
                        }
                    }
                };

                let result =
                    retry(create_fut, interval(Duration::from_secs(10000)).attempts(2)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
                assert!(result.is_ok());
            })
        }

        #[test]
        fn should_return_last_error_when_all_failed() {
            run_test(async move {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        if *mutex_guard == 1 {
                            Err::<(), _>(*mutex_guard)
                        } else {
                            Err(*mutex_guard)
                        }
                    }
                };

                let result =
                    retry(create_fut, interval(Duration::from_secs(10000)).attempts(2)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 3);
                assert_eq!(result, Err(3));
            })
        }

        #[test]
        fn should_retry_after_delay() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
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
                    }
                };
                let now = Instant::now();
                let result =
                    retry(create_fut, interval(Duration::from_millis(50)).attempts(2)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
                assert!(now.elapsed() >= Duration::from_millis(50));
                assert!(result.is_ok());
            })
        }

        #[test]
        fn should_not_retry_when_ok() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    Box::pin(async move {
                        crate::tests::yield_now().await;

                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        Ok::<_, ()>(())
                    })
                };

                let result =
                    retry(create_fut, interval(Duration::from_secs(10000)).attempts(2)).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 1);
                assert!(result.is_ok());
            })
        }
    }
}

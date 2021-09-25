use std::future::{ready, Ready};

use crate::Policy;

use super::Attempts;

/// Creates a policy to run futures concurrently until [`ConcurrentRetry::retry`] returns [`None`].
/// ## Example
/// Sends at most 4 concurrent requests and waits for one for which result [`ConcurrentRetry::retry`] returns [`Some`].
///
/// If one of the futures completes immediately (without returning [`std::task::Poll::Pending`]) no additional futures will be run
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
pub fn parallel<P>(policy: P) -> ParallelRetryPolicy<P> {
    ParallelRetryPolicy { policy }
}

/// A helper policy trait which is used for [`interval`] and [`parallel`] functions.
pub trait ConcurrentRetry<T, E>: Sized {
    /// Checks the policy if a new futures should be created.
    ///
    /// This method is passed a reference to the future's result or [`None`] if there is no completed future.
    ///
    /// If a new future should be created and polled return [`Some`] with a new policy, otherwise return [`None`].
    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self>;
}

impl<T, E> ConcurrentRetry<T, E> for Attempts<T, E> {
    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self> {
        self.retry(result)
    }
}

/// A policy is created by [`parallel`] function
#[derive(Debug, Clone, Copy)]
pub struct ParallelRetryPolicy<P> {
    policy: P,
}

impl<P, T, E> Policy<T, E> for ParallelRetryPolicy<P>
where
    P: ConcurrentRetry<T, E>,
{
    type ForceRetryFuture = Ready<()>;
    type RetryFuture = Ready<Self>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        ready(())
    }

    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        Some(ready(Self {
            policy: self.policy.retry(result)?,
        }))
    }
}
#[cfg(any(feature = "tokio", feature = "async-std"))]
mod delayed {
    use super::*;
    use std::time::Duration;

    /// Creates a policy to run additional future every time when `force_retry_after` delay elapsed and/or [`ConcurrentRetry::retry`] returns [`None`].
    ///
    /// After each completed future the previous delay is dropped and a new one started.
    /// ## Example
    /// Sends at most 4 concurrent requests and waits for one for which result [`ConcurrentRetry::retry`] returns [`Some`].
    ///
    /// Every next future will be run only after 1 second.
    ///
    /// If request time takes less than 1 second no additional futures will be run.
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
    pub fn interval<P>(policy: P, force_retry_after: Duration) -> IntervalRetryPolicy<P> {
        IntervalRetryPolicy {
            policy,
            force_retry_after,
        }
    }

    /// A policy is created by [`interval`] function
    pub struct IntervalRetryPolicy<P> {
        policy: P,
        force_retry_after: Duration,
    }

    impl<P, T, E> Policy<T, E> for IntervalRetryPolicy<P>
    where
        P: ConcurrentRetry<T, E>,
    {
        type ForceRetryFuture = crate::sleep::Sleep;
        type RetryFuture = Ready<Self>;

        fn force_retry_after(&self) -> Self::ForceRetryFuture {
            crate::sleep::sleep(self.force_retry_after)
        }

        fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
            Some(ready(Self {
                policy: self.policy.retry(result)?,
                force_retry_after: self.force_retry_after,
            }))
        }
    }
}
#[cfg(any(feature = "tokio", feature = "async-std"))]
pub use delayed::*;

#[cfg(test)]
mod test {
    use std::sync::{Arc, Mutex};

    use super::super::Attempts;
    use crate::retry;
    use crate::tests::run_test;
    use std::future::pending;

    mod concurrent {

        use crate::policies::concurrent::parallel;

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

                let result = retry(create_fut, parallel(Attempts::new(2))).await;

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

                let result = retry(create_fut, parallel(Attempts::new(2))).await;

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

                let result = retry(create_fut, parallel(Attempts::new(2))).await;

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

                let result = retry(
                    create_fut,
                    interval(Attempts::new(2), Duration::from_secs(10000)),
                )
                .await;

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

                let result = retry(
                    create_fut,
                    interval(Attempts::new(2), Duration::from_secs(10000)),
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
                let result = retry(
                    create_fut,
                    interval(Attempts::new(2), Duration::from_millis(50)),
                )
                .await;

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

                let result = retry(
                    create_fut,
                    interval(Attempts::new(2), Duration::from_secs(10000)),
                )
                .await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 1);
                assert!(result.is_ok());
            })
        }
    }
}
